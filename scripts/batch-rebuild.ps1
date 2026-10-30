# batch-rebuild.ps1 [-Out folder]
#
# batch-rebuild.sh for Windows: builds every game already in the output
# folder, build\ by default, again with this kit, as after an update or with
# the title.hints a handheld wrote.
param(
    [string]$Out = ""
)

$repo = Split-Path -Parent $PSScriptRoot

# each game builds from the copy the last build left, moved out of the way
# while the build copies it back, and put back if it did not, also when a
# build stopped halfway left it there
if (-not $Out) { $Out = Join-Path $repo "build" }
$games = Join-Path $Out "Pocket Native\pocket-native\games"
$staging = Join-Path $repo ".update\games"
function Restore {
    foreach ($copy in @(Get-ChildItem -LiteralPath $staging -Filter *.3ds -ErrorAction SilentlyContinue)) {
        $game = Join-Path $games "$($copy.BaseName)\game.3ds"
        $built = Get-Item -LiteralPath $game -ErrorAction SilentlyContinue
        if ($built -and $built.Length -eq $copy.Length) {
            Remove-Item -LiteralPath $copy.FullName
        } else {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $game) | Out-Null
            Move-Item -Force -LiteralPath $copy.FullName -Destination $game
        }
    }
}
Restore
$built = @(Get-ChildItem -Path (Join-Path $games "*\game.3ds") -ErrorAction SilentlyContinue)
if ($built.Count -eq 0) {
    Write-Host "== no games in $Out to build again, build them as before"
    exit 0
}
New-Item -ItemType Directory -Force -Path $staging | Out-Null
foreach ($game in $built) {
    Move-Item -LiteralPath $game.FullName -Destination (Join-Path $staging "$($game.Directory.Name).3ds")
}
Write-Host "== building $($built.Count) game$(if ($built.Count -ne 1) { "s" }) again"
$status = 1
try {
    $roms = @(Get-ChildItem -LiteralPath $staging -Filter *.3ds | ForEach-Object { $_.FullName })
    & (Join-Path $repo "scripts\build.ps1") @roms -Out $Out
    $status = $LASTEXITCODE
} finally {
    Restore
    if (-not (Get-ChildItem -LiteralPath $staging)) { Remove-Item -Recurse -Force -LiteralPath (Join-Path $repo ".update") }
}
if ($status -eq 0) { Write-Host "== done: copy `"Pocket Native`" onto the card again, over the old one" }
exit $status
