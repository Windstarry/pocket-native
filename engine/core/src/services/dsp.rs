//! dsp::DSP, the audio DSP.

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::kernel::object::{KObject, ObjectId};
use crate::kernel::sync::ResetType;
use crate::System;

/// command ids from the DSP service table.
mod command {
    pub const RECV_DATA: u16 = 0x0001;
    pub const RECV_DATA_IS_READY: u16 = 0x0002;
    pub const SEND_DATA: u16 = 0x0003;
    pub const SEND_DATA_IS_EMPTY: u16 = 0x0004;
    pub const SEND_FIFO_EX: u16 = 0x0005;
    pub const RECV_FIFO_EX: u16 = 0x0006;
    pub const SET_SEMAPHORE: u16 = 0x0007;
    pub const GET_SEMAPHORE: u16 = 0x0008;
    pub const CLEAR_SEMAPHORE: u16 = 0x0009;
    pub const MASK_SEMAPHORE: u16 = 0x000A;
    pub const CHECK_SEMAPHORE_REQUEST: u16 = 0x000B;
    pub const CONVERT_PROCESS_ADDRESS_FROM_DSP_DRAM: u16 = 0x000C;
    pub const WRITE_PROCESS_PIPE: u16 = 0x000D;
    pub const READ_PIPE: u16 = 0x000E;
    pub const GET_PIPE_READABLE_SIZE: u16 = 0x000F;
    pub const READ_PIPE_IF_POSSIBLE: u16 = 0x0010;
    pub const LOAD_COMPONENT: u16 = 0x0011;
    pub const UNLOAD_COMPONENT: u16 = 0x0012;
    pub const FLUSH_DATA_CACHE: u16 = 0x0013;
    pub const INVALIDATE_DATA_CACHE: u16 = 0x0014;
    pub const REGISTER_INTERRUPT_EVENTS: u16 = 0x0015;
    pub const GET_SEMAPHORE_EVENT_HANDLE: u16 = 0x0016;
    pub const SET_SEMAPHORE_MASK: u16 = 0x0017;
    pub const GET_HEADPHONE_STATUS: u16 = 0x001F;
    pub const FORCE_HEADPHONE_OUT: u16 = 0x0020;
    pub const GET_IS_DSP_OCCUPIED: u16 = 0x0021;
}

/// the pipe a title uses to drive the audio firmware.
const PIPE_AUDIO: u32 = 2;
/// the pipe its AAC decoder takes requests on.
const PIPE_BINARY: u32 = 3;
const PIPE_COUNT: usize = 8;

/// what the title writes to the audio pipe to change the firmware's state.
const STATE_INITIALIZE: u8 = 0;
const STATE_SHUTDOWN: u8 = 1;
const STATE_WAKEUP: u8 = 2;
const STATE_SLEEP: u8 = 3;

/// addresses in DSP data memory of the structures the firmware exposes.
mod layout {
    pub const FRAME_COUNTER: u16 = 0xBFFF;
    pub const SOURCE_CONFIGURATIONS: u16 = 0x9E92;
    pub const SOURCE_STATUSES: u16 = 0x8680;
    pub const ADPCM_COEFFICIENTS: u16 = 0xA792;
    pub const DSP_CONFIGURATION: u16 = 0x9430;
    pub const DSP_STATUS: u16 = 0x8400;
    pub const FINAL_SAMPLES: u16 = 0x8540;
    pub const INTERMEDIATE_MIX_SAMPLES: u16 = 0x9492;
    pub const COMPRESSOR: u16 = 0x8710;
    pub const DSP_DEBUG: u16 = 0x8410;
    pub const UNKNOWN_10: u16 = 0xA912;
    pub const UNKNOWN_11: u16 = 0xAA12;
    pub const UNKNOWN_12: u16 = 0xAAD2;
    pub const UNKNOWN_13: u16 = 0xAC52;
    pub const UNKNOWN_14: u16 = 0xAC5C;
}

#[derive(Default)]
pub struct DspState {
    pub component_loaded: bool,
    pub running: bool,
    pub semaphore_event_object: Option<ObjectId>,
    /// events registered for the three DSP interrupts.
    pub interrupt_events: Vec<ObjectId>,
    pub semaphore_mask: u32,
    /// what GetSemaphore reports.
    pub semaphore_value: u32,
    /// data waiting to be read out of each pipe.
    pipes: [Vec<u8>; PIPE_COUNT],
    /// the 24 voices the audio firmware plays.
    voices: super::dsp_voices::Voices,
    mixer: super::dsp_mixer::Mixer,
    /// what came out of the speakers since the frontend last took it.
    pub output: Vec<[i16; 2]>,
    /// the thread mixing frames, when there is one.
    sound_thread: Option<SoundThread>,
    pub(crate) aac: super::dsp_aac::Aac,
}

/// the most output kept for a frontend that is not taking it, a second.
const MAX_OUTPUT: usize = 32_768;

/// the DSP's mixing on a thread of its own, as the console's DSP mixes
/// beside its CPU: a frame goes out to it once the voices played it, which
/// is all a title reads back, and comes back mixed by the next.
struct SoundThread {
    jobs: std::sync::mpsc::Sender<(Vec<super::dsp_voices::VoiceMix>, super::dsp_mixer::Levels)>,
    frames: std::sync::mpsc::Receiver<super::dsp_voices::Frame>,
    /// the region the frame out on the thread is played into, when one is.
    out: Option<u32>,
}

/// mixes frames on a thread of its own from here on.
pub fn start_sound_thread(dsp: &mut DspState) {
    if dsp.sound_thread.is_some() {
        return;
    }
    let (jobs, receiver) = std::sync::mpsc::channel::<(Vec<super::dsp_voices::VoiceMix>, super::dsp_mixer::Levels)>();
    let (done, frames) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("sound".into()).spawn(move || {
        for (mixing, levels) in receiver {
            if done.send(super::dsp_mixer::final_mix(&super::dsp_voices::mix(&mixing), levels)).is_err() {
                return;
            }
        }
    });
    match spawned {
        Ok(_) => dsp.sound_thread = Some(SoundThread { jobs, frames, out: None }),
        Err(error) => log::warn!("no sound thread, mixing on the title's, {error}"),
    }
}

/// takes the frame the sound thread mixed, when one is out.
pub fn collect(system: &mut System) {
    let Some(thread) = &mut system.services.dsp.sound_thread else { return };
    let Some(write) = thread.out.take() else { return };
    let Ok(frame) = thread.frames.recv() else { return };
    super::dsp_mixer::write_final(&mut system.memory, mixer_layout(), write, &frame);
    play(&mut system.services.dsp, &frame);
}

fn mixer_layout() -> super::dsp_mixer::Layout {
    super::dsp_mixer::Layout {
        configuration: dsp_dram_to_arm(layout::DSP_CONFIGURATION),
        final_samples: dsp_dram_to_arm(layout::FINAL_SAMPLES),
        intermediate_samples: dsp_dram_to_arm(layout::INTERMEDIATE_MIX_SAMPLES),
    }
}

/// what the speakers play, kept for the frontend.
fn play(dsp: &mut DspState, frame: &super::dsp_voices::Frame) {
    dsp.output.extend_from_slice(frame);
    if dsp.output.len() > MAX_OUTPUT {
        let excess = dsp.output.len() - MAX_OUTPUT;
        dsp.output.drain(..excess);
    }
}

impl DspState {
    fn reset_pipes(&mut self) {
        for pipe in &mut self.pipes {
            pipe.clear();
        }
    }

    /// queues the reply the real firmware sends after initialization, the
    /// count of structures it exposes, then where each one lives.
    fn write_struct_addresses(&mut self) {
        const ADDRESSES: [u16; 15] = [
            layout::FRAME_COUNTER,
            layout::SOURCE_CONFIGURATIONS,
            layout::SOURCE_STATUSES,
            layout::ADPCM_COEFFICIENTS,
            layout::DSP_CONFIGURATION,
            layout::DSP_STATUS,
            layout::FINAL_SAMPLES,
            layout::INTERMEDIATE_MIX_SAMPLES,
            layout::COMPRESSOR,
            layout::DSP_DEBUG,
            layout::UNKNOWN_10,
            layout::UNKNOWN_11,
            layout::UNKNOWN_12,
            layout::UNKNOWN_13,
            layout::UNKNOWN_14,
        ];

        let mut response = Vec::with_capacity(2 + ADDRESSES.len() * 2);
        response.extend_from_slice(&(ADDRESSES.len() as u16).to_le_bytes());
        for address in ADDRESSES {
            response.extend_from_slice(&address.to_le_bytes());
        }
        self.pipes[PIPE_AUDIO as usize] = response;
    }

    fn read_pipe(&mut self, pipe: u32, length: usize) -> Vec<u8> {
        let Some(data) = self.pipes.get_mut(pipe as usize) else {
            return Vec::new();
        };
        let count = length.min(data.len());
        data.drain(..count).collect()
    }

    fn readable(&self, pipe: u32) -> usize {
        self.pipes.get(pipe as usize).map_or(0, |d| d.len())
    }
}

pub fn handle(system: &mut System, buffer: &CommandBuffer, header: Header) -> bool {
    use command::*;
    let id = header.command_id();
    match id {
        // RecvData(register) -> the DSP's reply.
        RECV_DATA => {
            let value = if system.services.dsp.running { 0 } else { 1 };
            buffer.reply(&mut system.memory, id, &[value]);
            true
        }
        RECV_DATA_IS_READY => {
            buffer.reply(&mut system.memory, id, &[1]);
            true
        }
        SEND_DATA | SEND_FIFO_EX | SET_SEMAPHORE | CLEAR_SEMAPHORE | MASK_SEMAPHORE => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        SEND_DATA_IS_EMPTY | CHECK_SEMAPHORE_REQUEST => {
            buffer.reply(&mut system.memory, id, &[1]);
            true
        }
        GET_SEMAPHORE => {
            buffer.reply(&mut system.memory, id, &[system.services.dsp.semaphore_value]);
            true
        }
        RECV_FIFO_EX => {
            buffer.reply(&mut system.memory, id, &[0]);
            true
        }
        // ConvertProcessAddressFromDspDram(address) -> the same address in the
        // caller's space. DSP memory is mapped one to one for us.
        CONVERT_PROCESS_ADDRESS_FROM_DSP_DRAM => {
            let address = buffer.get(&mut system.memory, 1);
            let converted =
                zakuro_common::memory_map::DSP_RAM_VADDR + (address << 1) + 0x40000;
            buffer.reply(&mut system.memory, id, &[converted]);
            true
        }
        // WriteProcessPipe(channel, size, <buffer>)
        WRITE_PROCESS_PIPE => {
            let channel = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let source = buffer.get(&mut system.memory, 4);
            let mut data = vec![0u8; size.min(0x1000) as usize];
            system.memory.read_bytes(source, &mut data);
            log::debug!("dsp: pipe {channel} takes {size} bytes {:02X?}", &data[..data.len().min(32)]);
            write_pipe(system, channel, &data);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // ReadPipe / ReadPipeIfPossible(channel, peer, size)
        READ_PIPE | READ_PIPE_IF_POSSIBLE => {
            let channel = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 3) & 0xFFFF;

            let tls = system.kernel.current().map_or(0, |t| t.tls);
            let (destination, capacity) = buffer.static_buffer(&mut system.memory, tls, 0);
            let wanted = size.min(capacity.max(size)) as usize;

            let data = system.services.dsp.read_pipe(channel, wanted);
            let count = data.len() as u32;
            if destination != 0 && !data.is_empty() {
                system.memory.write_bytes(destination, &data);
            }
            log::debug!(
                "dsp: pipe {channel} gave {count} of {size} bytes to 0x{destination:08X}"
            );

            buffer.set(&mut system.memory, 0, Header::new(id, 2, 2).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, count);
            buffer.set(&mut system.memory, 3, Descriptor::static_buffer(count, 0));
            buffer.set(&mut system.memory, 4, destination);
            true
        }

        // GetPipeReadableSize(channel, peer)
        GET_PIPE_READABLE_SIZE => {
            let channel = buffer.get(&mut system.memory, 1);
            let size = system.services.dsp.readable(channel) as u32;
            buffer.reply(&mut system.memory, id, &[size]);
            true
        }
        // LoadComponent(size, program mask, data mask, <buffer>)
        LOAD_COMPONENT => {
            system.services.dsp.component_loaded = true;
            log::info!("dsp: the title uploaded a firmware component; using the HLE audio path");
            // the second word says whether the component was accepted.
            buffer.set(&mut system.memory, 0, Header::new(id, 2, 2).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, 1);
            buffer.set(&mut system.memory, 3, (0x100000 << 4) | 0xA);
            buffer.set(&mut system.memory, 4, 0);
            true
        }
        UNLOAD_COMPONENT => {
            system.services.dsp.component_loaded = false;
            system.services.dsp.running = false;
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        FLUSH_DATA_CACHE | INVALIDATE_DATA_CACHE => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // RegisterInterruptEvents(interrupt, channel, event handle)
        REGISTER_INTERRUPT_EVENTS => {
            let handle = buffer.get(&mut system.memory, 4);
            // kept until the title is done, whatever it does with its handle
            if let Some(object) = system.kernel.resolve(handle) {
                if !system.services.dsp.interrupt_events.contains(&object) {
                    system.kernel.objects.add_ref(object);
                    system.services.dsp.interrupt_events.push(object);
                }
            }
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // GetSemaphoreEventHandle -> the event the audio thread waits on.
        GET_SEMAPHORE_EVENT_HANDLE => {
            let make = || KObject::Event(crate::kernel::sync::Event::new(ResetType::OneShot, "DSP:semaphore"));
            let handle = system.kernel.service_handle(&mut system.services.dsp.semaphore_event_object, "DSP:semaphore", make);
            buffer.set(&mut system.memory, 0, Header::new(id, 1, 2).0);
            buffer.set(&mut system.memory, 1, 0);
            buffer.set(&mut system.memory, 2, Descriptor::handles(1));
            buffer.set(&mut system.memory, 3, handle);
            true
        }
        SET_SEMAPHORE_MASK => {
            system.services.dsp.semaphore_mask = buffer.get(&mut system.memory, 1);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // no headphones are plugged into an emulator.
        GET_HEADPHONE_STATUS => {
            buffer.reply(&mut system.memory, id, &[0]);
            true
        }
        FORCE_HEADPHONE_OUT => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        GET_IS_DSP_OCCUPIED => {
            buffer.reply(&mut system.memory, id, &[0]);
            true
        }
        _ => false,
    }
}

/// handles a write to a pipe.
fn write_pipe(system: &mut System, channel: u32, data: &[u8]) {
    // a request for the AAC decoder, answered at once
    if channel == PIPE_BINARY && !data.is_empty() {
        let answer = super::dsp_aac::answer(system, data);
        system.services.dsp.pipes[PIPE_BINARY as usize] = answer.to_vec();
        signal_semaphore(system);
        return;
    }
    if channel != PIPE_AUDIO || data.is_empty() {
        return;
    }
    match data[0] {
        STATE_INITIALIZE => {
            system.services.dsp.reset_pipes();
            system.services.dsp.write_struct_addresses();
            system.services.dsp.running = true;
            log::debug!("dsp: initialized");
        }
        STATE_SHUTDOWN => {
            system.services.dsp.reset_pipes();
            system.services.dsp.running = false;
            log::debug!("dsp: shut down");
        }
        STATE_WAKEUP => system.services.dsp.running = true,
        STATE_SLEEP => system.services.dsp.running = false,
        other => log::debug!("dsp: unknown audio pipe state change {other}"),
    }
    // the firmware answers every state change by signalling the semaphore.
    signal_semaphore(system);
}

/// converts a DSP data-memory address into the address the ARM side sees.
fn dsp_dram_to_arm(address: u16) -> u32 {
    zakuro_common::memory_map::DSP_RAM_VADDR + ((address as u32) << 1) + 0x40000
}

/// runs one audio frame of the firmware, plays the voices through their
/// buffers and publishes their statuses, which is how a title's sound
/// library knows its audio is moving, then mixes what they played.
pub fn advance(system: &mut System) {
    if !system.services.dsp.running {
        return;
    }
    let voices = super::dsp_voices::Layout {
        configurations: dsp_dram_to_arm(layout::SOURCE_CONFIGURATIONS),
        statuses: dsp_dram_to_arm(layout::SOURCE_STATUSES),
        coefficients: dsp_dram_to_arm(layout::ADPCM_COEFFICIENTS),
        frame_counter: dsp_dram_to_arm(layout::FRAME_COUNTER),
    };
    let mixer = mixer_layout();
    collect(system);
    let read = super::dsp_voices::current_region(&mut system.memory, voices);
    let dsp = &mut system.services.dsp;
    let mixing = dsp.voices.tick(&mut system.memory, voices);
    dsp.mixer.read_config(&mut system.memory, mixer, read);
    // the mixes a title processes itself go through its memory now, the
    // rest can be mixed while it goes on
    if let Some(thread) = dsp.sound_thread.as_mut().filter(|_| !dsp.mixer.uses_aux()) {
        if thread.jobs.send((mixing, dsp.mixer.levels())).is_ok() {
            thread.out = Some(1 - read);
        }
        return;
    }
    let frame = dsp.mixer.finish(&mut system.memory, mixer, read, super::dsp_voices::mix(&mixing));
    play(dsp, &frame);
}

/// signals the DSP's interrupt events, as the firmware does at the end of
/// every audio frame.
pub fn signal_semaphore(system: &mut System) {
    // real firmware always reports the requested mask back as "satisfied" when
    // it signals the event, a driver that reads 0 here after waking up sees a
    // spurious wakeup and goes straight back to sleep.
    system.services.dsp.semaphore_value = system.services.dsp.semaphore_mask;

    if let Some(object) = system.services.dsp.semaphore_event_object {
        system.kernel.signal_event(object);
    }
    let events = system.services.dsp.interrupt_events.clone();
    for object in events {
        system.kernel.signal_event(object);
    }
}

#[cfg(test)]
mod tests {
    use super::command::*;
    use super::*;
    use crate::kernel::ipc::Header;
    use crate::{Config, System};

    /// the command ids are pinned by the exact headers a title sends.
    #[test]
    fn command_ids_match_the_headers_titles_send() {
        assert_eq!(Header::new(LOAD_COMPONENT, 3, 2).0, 0x0011_00C2);
        assert_eq!(Header::new(REGISTER_INTERRUPT_EVENTS, 2, 2).0, 0x0015_0082);
        assert_eq!(Header::new(GET_SEMAPHORE_EVENT_HANDLE, 0, 0).0, 0x0016_0000);
        assert_eq!(Header::new(WRITE_PROCESS_PIPE, 2, 2).0, 0x000D_0082);
        assert_eq!(Header::new(READ_PIPE_IF_POSSIBLE, 3, 2).0, 0x0010_00C2);
    }

    /// real firmware reports the mask a driver registered with
    /// SetSemaphoreMask back as satisfied whenever it fires the semaphore
    /// event, and a driver that reads back 0 here treats the wakeup as
    /// spurious.
    #[test]
    fn signalling_the_semaphore_reports_the_registered_mask() {
        let mut system = System::new(Config::default());
        system.services.dsp.semaphore_mask = 0x2FFF;
        assert_eq!(system.services.dsp.semaphore_value, 0);

        signal_semaphore(&mut system);

        assert_eq!(system.services.dsp.semaphore_value, 0x2FFF);
    }
}
