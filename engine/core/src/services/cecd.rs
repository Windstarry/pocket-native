//! cecd:u, StreetPass. the daemon keeps each title's box in files, as the
//! console does in its CEC folder, but never meets another console, so
//! nothing arrives in an inbox and nothing leaves an outbox. a title that
//! turns StreetPass on reads back the box list and boxes it wrote.

use std::path::PathBuf;

use zakuro_common::result::{errors, Level, Module, ResultCode, Summary};

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::kernel::object::{KObject, ObjectId};
use crate::kernel::sync::ResetType;
use crate::System;

/// the daemon's states as GetCecdState reports them.
const WORKING: u32 = 0;
const IDLE: u32 = 1;

/// a box or file that isn't there, and reading or writing a directory.
const NO_DATA: u32 = ResultCode::new(Level::Status, Summary::NotFound, Module::Cec, 1007).0;
const NOT_AUTHORIZED: u32 = ResultCode::new(Level::Status, Summary::NotFound, Module::Cec, 1002).0;

/// the open mode's bits, make a missing directory, and put the daemon's
/// own lists into what is written.
const CREATE: u32 = 8;
const CHECK: u32 = 16;

/// what a request names, a file or directory of the CEC folder.
const MBOX_LIST: u32 = 1;
const MBOX_INFO: u32 = 2;
const INBOX_INFO: u32 = 3;
const OUTBOX_INFO: u32 = 4;
const OUTBOX_INDEX: u32 = 5;
const INBOX_MESSAGE: u32 = 6;
const OUTBOX_MESSAGE: u32 = 7;
const ROOT_DIR: u32 = 10;
const MBOX_DIR: u32 = 11;
const INBOX_DIR: u32 = 12;
const OUTBOX_DIR: u32 = 13;
const MBOX_PROGRAM_ID: u32 = 150;

/// the box list, a header and 24 names of 16 bytes, the title's id in hex
/// and zeros.
const MBOX_LIST_SIZE: usize = 0x18C;
const MAX_BOXES: usize = 24;
/// a box's info header, then the headers of its messages.
const BOX_INFO_SIZE: usize = 0x20;
const MESSAGE_HEADER_SIZE: usize = 0x70;
/// where a message keeps its id, and how long the id is.
const MESSAGE_ID: usize = 0x20;
const MESSAGE_ID_SIZE: usize = 8;

/// what ReadData answers about the system.
const EULA_VERSION_INFO: u32 = 1;
const EULA_INFO: u32 = 2;
const PARENTAL_CONTROLS_INFO: u32 = 3;

pub struct CecdState {
    state: u32,
    /// the info changed and state changed events.
    events: Option<[ObjectId; 2]>,
    /// the program id, path and open mode the last Open was for, which the
    /// Read and Write after it go to.
    opened: Option<(u32, u32, u32)>,
}

impl Default for CecdState {
    fn default() -> Self {
        CecdState { state: IDLE, events: None, opened: None }
    }
}

pub fn handle(system: &mut System, buffer: &CommandBuffer, header: Header) -> bool {
    let command = header.command_id();
    let word = |system: &mut System, index: u32| buffer.get(&mut system.memory, index);
    match command {
        // Open(program id, path, mode, pid), a directory answers how many
        // entries it has and a file its size
        0x0001 => {
            let (program, kind, mode) = (word(system, 1), word(system, 2), word(system, 3));
            system.services.cecd.opened = Some((program, kind, mode));
            let path = path(system, program, kind, &[]);
            let (result, value) = if is_directory(kind) {
                match std::fs::read_dir(&path) {
                    Ok(entries) => (0, entries.count() as u32),
                    Err(_) if mode & CREATE != 0 && std::fs::create_dir_all(&path).is_ok() => (0, 0),
                    Err(_) => (NO_DATA, 0),
                }
            } else {
                // the daemon makes the file when its box is there
                let made = std::fs::OpenOptions::new().create(true).append(true).open(&path);
                match made.and_then(|file| file.metadata()) {
                    Ok(metadata) => {
                        if kind == MBOX_PROGRAM_ID {
                            let id = system.title.as_ref().map_or(0, |title| title.program_id());
                            let _ = std::fs::write(&path, id.to_le_bytes());
                        }
                        (0, metadata.len() as u32)
                    }
                    Err(_) => (NO_DATA, 0),
                }
            };
            reply(system, buffer, header, result, &[value]);
        }
        // Read(size, buffer) and Write(size, buffer), of what Open opened
        0x0002 | 0x0005 => {
            let size = word(system, 1);
            let Some((program, kind, mode)) = system.services.cecd.opened else {
                let values: &[u32] = if command == 0x0002 { &[0] } else { &[] };
                reply(system, buffer, header, NO_DATA, values);
                return true;
            };
            if command == 0x0002 {
                let (result, read) = read(system, buffer, header, program, kind, size, 0);
                reply(system, buffer, header, result, &[read]);
            } else {
                let result = write(system, buffer, header, program, kind, mode, size, 0, &[]);
                reply(system, buffer, header, result, &[]);
            }
        }
        // ReadMessage(program id, outbox, id size, size, id, buffer), and
        // with an HMAC key between the id and the buffer, which only the
        // console receiving the message checks
        0x0003 | 0x0004 => {
            let (program, kind, id) = message(system, buffer, header, 0);
            let size = word(system, 4);
            let last = mapped_buffers(system, buffer, header).len().saturating_sub(1);
            let (result, read) = read_path(system, buffer, header, path(system, program, kind, &id), size, last);
            reply(system, buffer, header, result, &[read]);
        }
        // WriteMessage(program id, outbox, id size, size, message, id), and
        // with an HMAC key between them
        0x0006 | 0x0007 => {
            let last = mapped_buffers(system, buffer, header).len().saturating_sub(1);
            let (program, kind, id) = message(system, buffer, header, last);
            let size = word(system, 4);
            let result = write(system, buffer, header, program, kind, 0, size, 0, &id);
            reply(system, buffer, header, result, &[]);
        }
        // Delete(program id, path, outbox, id size, id), a directory with
        // what is in it, or a message when there is an id
        0x0008 => {
            let (program, kind) = (word(system, 1), word(system, 2));
            let (_, message_kind, id) = message(system, buffer, header, 0);
            let path = match id.is_empty() {
                true => path(system, program, kind, &[]),
                false => path(system, program, message_kind, &id),
            };
            let result = if is_directory(kind) {
                std::fs::remove_dir_all(&path).map_err(|_| errors::FS_PATH_NOT_FOUND.0)
            } else {
                std::fs::remove_file(&path).map_err(|_| errors::FS_FILE_NOT_FOUND.0)
            };
            reply(system, buffer, header, result.err().unwrap_or(0), &[]);
        }
        // SetData(program id, size, option, buffer), option 2 writes the
        // outbox's index
        0x0009 => {
            let (program, size, option) = (word(system, 1), word(system, 2), word(system, 3));
            if option == 2 && size > 0 {
                write(system, buffer, header, program, OUTBOX_INDEX, CHECK, size, 0, &[]);
            }
            reply(system, buffer, header, 0, &[]);
        }
        // ReadData(size, what, parameter size, parameter, buffer)
        0x000A => {
            let (size, what) = (word(system, 1), word(system, 2));
            let data: &[u8] = match what {
                // the agreement cfg reports, and that it was accepted
                EULA_VERSION_INFO => &super::cfg::EULA_VERSION,
                EULA_INFO => &[1],
                PARENTAL_CONTROLS_INFO => &[0],
                _ => &[],
            };
            if let Some(&(descriptor, address)) = mapped_buffers(system, buffer, header).get(1) {
                let count = data.len().min(size as usize).min(buffer_size(descriptor));
                system.memory.write_bytes(address, &data[..count]);
            }
            reply(system, buffer, header, 0, &[]);
        }
        // Start and Stop, a title waits for the state changed event after
        // either before it goes on
        0x000B | 0x000C => {
            system.services.cecd.state = if command == 0x000B { WORKING } else { IDLE };
            let changed = events(system)[1];
            system.kernel.signal_event(changed);
            buffer.reply(&mut system.memory, command, &[]);
        }
        // GetCecInfoBuffer(what, size, buffer), nothing new to tell
        0x000D => reply(system, buffer, header, 0, &[]),
        // GetCecdState
        0x000E => {
            let state = system.services.cecd.state;
            buffer.reply(&mut system.memory, command, &[state]);
        }
        // GetCecInfoEventHandle and GetChangeStateEventHandle
        0x000F | 0x0010 => {
            let object = events(system)[command as usize - 0x000F];
            let handle = system.kernel.handles.create(&mut system.kernel.objects, object, "cecd:u event");
            buffer.set(&mut system.memory, 0, Header::new(command, 1, 2).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, Descriptor::handles(1));
            buffer.set(&mut system.memory, 3, handle);
        }
        // OpenAndWrite(size, program id, path, mode, pid, buffer) and
        // OpenAndRead
        0x0011 | 0x0012 => {
            let (size, program, kind, mode) = (word(system, 1), word(system, 2), word(system, 3), word(system, 4));
            if command == 0x0011 {
                let result = write(system, buffer, header, program, kind, mode, size, 0, &[]);
                reply(system, buffer, header, result, &[]);
            } else {
                let (result, read) = read(system, buffer, header, program, kind, size, 0);
                reply(system, buffer, header, result, &[read]);
            }
        }
        _ => return false,
    }
    true
}

/// where a path of the CEC folder is on the host, in the game's data, the
/// folder made with an empty box list the first time.
fn path(system: &System, program: u32, kind: u32, id: &[u8]) -> PathBuf {
    let root = system.services.fs.user_dir.join("cec");
    let list = root.join("MBoxList____");
    if !list.exists() {
        let mut empty = vec![0; MBOX_LIST_SIZE];
        check_box_list(system, 0, &mut empty);
        let _ = std::fs::create_dir_all(&root).and_then(|()| std::fs::write(&list, empty));
    }
    let mbox = root.join(format!("{program:08x}"));
    match kind {
        MBOX_LIST => list,
        MBOX_INFO => mbox.join("MBoxInfo____"),
        INBOX_INFO => mbox.join("InBox___/BoxInfo_____"),
        OUTBOX_INFO => mbox.join("OutBox__/BoxInfo_____"),
        OUTBOX_INDEX => mbox.join("OutBox__/OBIndex_____"),
        INBOX_MESSAGE => mbox.join("InBox___").join(format!("_{}", base64(id))),
        OUTBOX_MESSAGE => mbox.join("OutBox__").join(format!("_{}", base64(id))),
        ROOT_DIR => root,
        MBOX_DIR => mbox,
        INBOX_DIR => mbox.join("InBox___"),
        OUTBOX_DIR => mbox.join("OutBox__"),
        _ => mbox.join(format!("MBoxData.{:03}", kind.wrapping_sub(100))),
    }
}

fn is_directory(kind: u32) -> bool {
    matches!(kind, ROOT_DIR | MBOX_DIR | INBOX_DIR | OUTBOX_DIR)
}

/// a message's file name, its id in base64 with the daemon's alphabet,
/// which has no slash, and no padding.
fn base64(id: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+-";
    id.chunks(3)
        .flat_map(|chunk| {
            let bits = chunk.iter().enumerate().fold(0u32, |bits, (i, &byte)| bits | (byte as u32) << (16 - 8 * i));
            (0..=chunk.len()).map(move |i| DIGITS[(bits >> (18 - 6 * i)) as usize & 63] as char)
        })
        .collect()
}

/// the program id, outbox or inbox and the id of the message a request is
/// about, the id from the mapped buffer at index.
fn message(system: &mut System, buffer: &CommandBuffer, header: Header, index: usize) -> (u32, u32, Vec<u8>) {
    let program = buffer.get(&mut system.memory, 1);
    let outbox = buffer.get(&mut system.memory, if header.command_id() == 0x0008 { 3 } else { 2 }) & 0xFF != 0;
    let size = buffer.get(&mut system.memory, if header.command_id() == 0x0008 { 4 } else { 3 }) as usize;
    let mut id = Vec::new();
    if let Some(&(descriptor, address)) = mapped_buffers(system, buffer, header).get(index) {
        id.resize(size.min(buffer_size(descriptor)).min(MESSAGE_ID_SIZE), 0);
        system.memory.read_bytes(address, &mut id);
    }
    (program, if outbox { OUTBOX_MESSAGE } else { INBOX_MESSAGE }, id)
}

/// reads a file into the mapped buffer at index, the result and how much
/// it read.
fn read(system: &mut System, buffer: &CommandBuffer, header: Header, program: u32, kind: u32, size: u32, index: usize) -> (u32, u32) {
    if is_directory(kind) {
        return (NOT_AUTHORIZED, 0);
    }
    read_path(system, buffer, header, path(system, program, kind, &[]), size, index)
}

fn read_path(system: &mut System, buffer: &CommandBuffer, header: Header, path: PathBuf, size: u32, index: usize) -> (u32, u32) {
    let Ok(data) = std::fs::read(&path) else {
        return (NO_DATA, 0);
    };
    let Some(&(descriptor, address)) = mapped_buffers(system, buffer, header).get(index) else {
        return (0, 0);
    };
    let count = data.len().min(size as usize).min(buffer_size(descriptor));
    system.memory.write_bytes(address, &data[..count]);
    (0, count as u32)
}

/// writes the mapped buffer at index over a file, with the daemon's lists
/// put in when the mode asks it to check, the result.
#[allow(clippy::too_many_arguments)]
fn write(system: &mut System, buffer: &CommandBuffer, header: Header, program: u32, kind: u32, mode: u32, size: u32, index: usize, id: &[u8]) -> u32 {
    if is_directory(kind) {
        return NOT_AUTHORIZED;
    }
    let mut data = Vec::new();
    if let Some(&(descriptor, address)) = mapped_buffers(system, buffer, header).get(index) {
        data.resize((size as usize).min(buffer_size(descriptor)), 0);
        system.memory.read_bytes(address, &mut data);
    }
    if mode & CHECK != 0 {
        check(system, program, kind, &mut data);
    }
    match std::fs::write(path(system, program, kind, id), data) {
        Ok(()) => 0,
        // its box isn't there
        Err(_) => NO_DATA,
    }
}

/// what the daemon puts right in a file a title writes with the check flag:
/// the file's magic, and the lists of boxes and messages it keeps itself.
fn check(system: &System, program: u32, kind: u32, data: &mut Vec<u8>) {
    let magic = match kind {
        MBOX_LIST => return check_box_list(system, program, data),
        MBOX_INFO => 0x6363,
        INBOX_INFO | OUTBOX_INFO => 0x6262,
        OUTBOX_INDEX => 0x6767,
        _ => return,
    };
    let minimum = if kind == OUTBOX_INDEX { 8 } else if kind == MBOX_INFO { 2 } else { BOX_INFO_SIZE };
    if data.len() < minimum {
        data.resize(minimum, 0);
    }
    data[..2].copy_from_slice(&u16::to_le_bytes(magic));
    if kind == MBOX_INFO {
        return;
    }
    let directory = path(system, program, if kind == INBOX_INFO { INBOX_DIR } else { OUTBOX_DIR }, &[]);
    let mut messages: Vec<Vec<u8>> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('_'))
        .filter_map(|entry| Some((entry.file_name(), std::fs::read(entry.path()).ok()?)))
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    if kind == OUTBOX_INDEX {
        // the header and the outbox's message ids
        data.truncate(8);
        data[4..8].copy_from_slice(&(messages.len() as u32).to_le_bytes());
        for message in &mut messages {
            message.resize(MESSAGE_ID + MESSAGE_ID_SIZE, 0);
            data.extend_from_slice(&message[MESSAGE_ID..MESSAGE_ID + MESSAGE_ID_SIZE]);
        }
    } else {
        // the header, its size, how many messages and theirs
        data.truncate(BOX_INFO_SIZE);
        data[0x14..0x18].copy_from_slice(&(messages.len() as u32).to_le_bytes());
        if data[0x18..0x1C] == [0; 4] {
            data.copy_within(0x10..0x14, 0x18);
        }
        for message in &mut messages {
            message.resize(MESSAGE_HEADER_SIZE, 0);
            data.extend_from_slice(&message[..MESSAGE_HEADER_SIZE]);
        }
        let size = data.len() as u32;
        data[4..8].copy_from_slice(&size.to_le_bytes());
    }
}

/// the box list's magic and version, with the title's box added, or for
/// program id 0 the boxes there are now.
fn check_box_list(system: &System, program: u32, data: &mut Vec<u8>) {
    data.resize(MBOX_LIST_SIZE, 0);
    if data[..2] != [0x68, 0x68] {
        data.fill(0);
        data[..2].copy_from_slice(b"hh");
    }
    data[4..8].copy_from_slice(&1u32.to_le_bytes());
    let count = (u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize).min(MAX_BOXES);
    let mut names: Vec<[u8; 8]> = (0..count).map(|i| data[0xC + 16 * i..0xC + 16 * i + 8].try_into().unwrap()).collect();
    if program != 0 {
        let name: [u8; 8] = format!("{program:08x}").into_bytes().try_into().unwrap();
        if !names.contains(&name) && names.len() < MAX_BOXES {
            names.push(name);
        }
    } else {
        let root = system.services.fs.user_dir.join("cec");
        let mut boxes: Vec<[u8; 8]> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok()?.into_bytes().try_into().ok())
            .collect();
        boxes.sort();
        boxes.truncate(MAX_BOXES);
        names = boxes;
    }
    data[8..12].copy_from_slice(&(names.len() as u32).to_le_bytes());
    data[0xC..].fill(0);
    for (i, name) in names.iter().enumerate() {
        data[0xC + 16 * i..0xC + 16 * i + 8].copy_from_slice(name);
    }
}

/// the request's mapped buffers, each its descriptor and address.
fn mapped_buffers(system: &mut System, buffer: &CommandBuffer, header: Header) -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    let mut at = 1 + header.normal_params();
    let end = at + header.translate_params();
    while at < end {
        let descriptor = buffer.get(&mut system.memory, at);
        at += match Descriptor::parse(descriptor) {
            Descriptor::Handles { count, .. } => 1 + count,
            Descriptor::MappedBuffer { .. } => {
                found.push((descriptor, buffer.get(&mut system.memory, at + 1)));
                2
            }
            _ => 2,
        };
    }
    found
}

fn buffer_size(descriptor: u32) -> usize {
    (descriptor >> 4) as usize
}

/// a reply with the result and values, then the request's mapped buffers
/// handed back.
fn reply(system: &mut System, buffer: &CommandBuffer, header: Header, result: u32, values: &[u32]) {
    let buffers = mapped_buffers(system, buffer, header);
    let normal = 1 + values.len() as u32;
    buffer.set(&mut system.memory, 0, Header::new(header.command_id(), normal, 2 * buffers.len() as u32).0);
    buffer.set(&mut system.memory, 1, result);
    for (i, &value) in values.iter().enumerate() {
        buffer.set(&mut system.memory, 2 + i as u32, value);
    }
    for (i, &(descriptor, address)) in buffers.iter().enumerate() {
        buffer.set(&mut system.memory, 1 + normal + 2 * i as u32, descriptor);
        buffer.set(&mut system.memory, 2 + normal + 2 * i as u32, address);
    }
}

/// the events, made the first time a title asks for one.
fn events(system: &mut System) -> [ObjectId; 2] {
    if let Some(events) = system.services.cecd.events {
        return events;
    }
    let events = ["cecd:u:InfoEvent", "cecd:u:ChangeStateEvent"].map(|name| {
        let object = system.kernel.objects.insert(KObject::Event(crate::kernel::sync::Event::new(ResetType::OneShot, name)));
        // the service keeps them, a title closing its handles must not take
        // them away
        system.kernel.objects.add_ref(object);
        object
    });
    system.services.cecd.events = Some(events);
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tests::system_with_thread;
    use crate::services::{handle_request, Target};

    /// the pid a request sends after its words.
    const PID: [u32; 2] = [0x20, 0];

    fn mapped(size: u32) -> u32 {
        (size << 4) | 0xC
    }

    fn request(system: &mut System, buffer: &CommandBuffer, command: u16, normal: &[u32], translate: &[u32]) -> u32 {
        buffer.set(&mut system.memory, 0, Header::new(command, normal.len() as u32, translate.len() as u32).0);
        for (i, &word) in normal.iter().chain(translate).enumerate() {
            buffer.set(&mut system.memory, 1 + i as u32, word);
        }
        handle_request(system, Target::service("cecd:u".into(), 0));
        buffer.get(&mut system.memory, 1)
    }

    fn system_in(name: &str) -> (System, CommandBuffer, PathBuf) {
        let (mut system, buffer) = system_with_thread();
        let dir = std::env::temp_dir().join(format!("zakuro-cecd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        system.services.fs.user_dir = dir.clone();
        (system, buffer, dir)
    }

    /// Kirby: Triple Deluxe turning StreetPass on: it deletes its box,
    /// rewrites the box list from the boxes there are and reads it back,
    /// which has to come back a box list.
    #[test]
    fn the_box_list_reads_back() {
        let (mut system, buffer, dir) = system_in("list");
        let at = buffer.address() + 0x100;
        assert_eq!(request(&mut system, &buffer, 0x0008, &[0x0010_BF00, MBOX_DIR, 0, 0], &[mapped(0), at]), errors::FS_PATH_NOT_FOUND.0);
        // Open(root, create), the box list is there
        assert_eq!(request(&mut system, &buffer, 0x0001, &[0, ROOT_DIR, CREATE], &PID), 0);
        assert_eq!(buffer.get(&mut system.memory, 2), 1);
        // a box of another title's
        std::fs::create_dir_all(dir.join("cec/00055d00")).unwrap();
        system.memory.write_bytes(at, &[0; MBOX_LIST_SIZE]);
        let size = MBOX_LIST_SIZE as u32;
        assert_eq!(request(&mut system, &buffer, 0x0011, &[size, 0, MBOX_LIST, 4 | CHECK], &[PID[0], PID[1], mapped(size), at]), 0);
        system.memory.write_bytes(at, &[0xFF; MBOX_LIST_SIZE]);
        assert_eq!(request(&mut system, &buffer, 0x0012, &[size, 0, MBOX_LIST, 2 | CHECK], &[PID[0], PID[1], mapped(size), at]), 0);
        assert_eq!(buffer.get(&mut system.memory, 2), size);
        let mut list = [0u8; MBOX_LIST_SIZE];
        system.memory.read_bytes(at, &mut list);
        assert_eq!(&list[..12], &[0x68, 0x68, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&list[0xC..0x1C], b"00055d00\0\0\0\0\0\0\0\0");
        assert!(system.services.unimplemented.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// a box made, a message written to its outbox, the outbox's info and
    /// index listing it, and the message read back and deleted.
    #[test]
    fn the_outbox_lists_its_messages() {
        let (mut system, buffer, dir) = system_in("outbox");
        let (at, id, out) = (buffer.address() + 0x100, buffer.address() + 0x200, buffer.address() + 0x300);
        let program = 0x0010_BF00;
        assert_eq!(request(&mut system, &buffer, 0x0001, &[program, OUTBOX_DIR, CREATE], &PID), 0);
        let mut message = vec![0u8; 0x80];
        message[..2].copy_from_slice(&[0x60, 0x60]);
        message[MESSAGE_ID..MESSAGE_ID + 8].copy_from_slice(b"MSGID001");
        system.memory.write_bytes(at, &message);
        system.memory.write_bytes(id, b"MSGID001");
        assert_eq!(request(&mut system, &buffer, 0x0006, &[program, 1, 8, 0x80], &[mapped(0x80), at, mapped(8), id]), 0);
        assert!(dir.join("cec/0010bf00/OutBox__").join(format!("_{}", base64(b"MSGID001"))).exists());

        system.memory.write_bytes(at, &[0; 0x20]);
        assert_eq!(request(&mut system, &buffer, 0x0011, &[0x20, program, OUTBOX_INFO, 4 | CHECK], &[PID[0], PID[1], mapped(0x20), at]), 0);
        let info = std::fs::read(dir.join("cec/0010bf00/OutBox__/BoxInfo_____")).unwrap();
        assert_eq!(info.len(), BOX_INFO_SIZE + MESSAGE_HEADER_SIZE);
        assert_eq!(&info[..2], &[0x62, 0x62]);
        assert_eq!(info[0x14], 1);

        system.memory.write_bytes(at, &[0; 8]);
        assert_eq!(request(&mut system, &buffer, 0x0009, &[program, 8, 2], &[mapped(8), at]), 0);
        let index = std::fs::read(dir.join("cec/0010bf00/OutBox__/OBIndex_____")).unwrap();
        assert_eq!(&index[..], b"gg\0\0\x01\0\0\0MSGID001");

        system.memory.write_bytes(out, &[0; 0x80]);
        assert_eq!(request(&mut system, &buffer, 0x0003, &[program, 1, 8, 0x80], &[mapped(8), id, mapped(0x80), out]), 0);
        assert_eq!(buffer.get(&mut system.memory, 2), 0x80);
        let mut read = vec![0u8; 0x80];
        system.memory.read_bytes(out, &mut read);
        assert_eq!(read, message);

        assert_eq!(request(&mut system, &buffer, 0x0008, &[program, OUTBOX_MESSAGE, 1, 8], &[mapped(8), id]), 0);
        assert_eq!(request(&mut system, &buffer, 0x0003, &[program, 1, 8, 0x80], &[mapped(8), id, mapped(0x80), out]), NO_DATA);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn message_names_are_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE");
        assert_eq!(base64(&[0xFB, 0xFF]), "+-8");
    }
}
