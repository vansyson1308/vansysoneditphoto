# Install MagicPic (app + magicpic-cli) on Windows from a GitHub release.
#
#   irm https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.ps1 | iex
#
# Options, as parameters (`powershell -ExecutionPolicy Bypass -File install.ps1 -Photos D:\Photos`)
# or, for `irm | iex`, as environment variables set first: MAGICPIC_VERSION (a tag such as
# v0.24.0), MAGICPIC_PHOTOS, MAGICPIC_PORT, MAGICPIC_DESKTOP_SHORTCUT=1, MAGICPIC_HOME.
#
# Downloads magicpic-windows-x86_64.zip and SHA256SUMS, refuses an archive whose checksum does
# not match, installs magicpic.exe and magicpic-cli.exe into %USERPROFILE%\.magicpic\bin, then runs
# `magicpic-cli setup`, which writes the photo folder, the launcher and config.json. Progress goes
# to stderr; stdout is setup's JSON with the absolute paths.
#
# Running it again updates in place while MagicPic and the MCP servers run: a running app is asked
# to quit first (never with unsaved changes: then nothing is installed), and a program still in
# use is renamed aside and replaced. It succeeds, or refuses before changing anything.
#
# Written for Windows PowerShell 5.1 in Constrained Language Mode: no New-Object, no .NET
# property setters or encodings, no script-written files. curl.exe and tar.exe (built into
# Windows 10 1803 and later) do the transfer and the unpacking; core cmdlets are the fallback.
param(
    [string]$Version = $env:MAGICPIC_VERSION,
    [string]$Repo = 'vansyson1308/vansysoneditphoto',
    [string]$From = '',
    [string]$Photos = $env:MAGICPIC_PHOTOS,
    [string]$Port = $env:MAGICPIC_PORT,
    [switch]$DesktopShortcut,
    [switch]$NoSetup
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Progress goes to stderr, so stdout holds only the JSON an agent reads: Write-Host lands on stdout
# when Windows PowerShell's output is redirected. Constrained Language Mode may refuse [Console];
# then the line goes to the host as before, ahead of the JSON.
function Say([string]$Text) {
    try { [Console]::Error.WriteLine("magicpic install: $Text") } catch { Write-Host "magicpic install: $Text" }
}
# `throw`, not `exit`: under `irm | iex` an exit would close the caller's PowerShell session.
function Fail([string]$Text) { throw "magicpic install: error: $Text" }

if (-not $Version) { $Version = 'latest' }
if ($env:MAGICPIC_DESKTOP_SHORTCUT -eq '1') { $DesktopShortcut = $true }
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') {
    if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
        Say 'ARM64 Windows: installing the x64 build, which runs under emulation.'
    } else {
        Fail "no prebuilt MagicPic for $($env:PROCESSOR_ARCHITECTURE): build from source (docs/agent-setup.md, Fallback)"
    }
}

$archive = 'magicpic-windows-x86_64.zip'
$home_ = $env:MAGICPIC_HOME
if (-not $home_) { $home_ = Join-Path $env:USERPROFILE '.magicpic' }
$bin = Join-Path $home_ 'bin'
$work = Join-Path $env:TEMP ('magicpic-install-' + $PID)
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null
New-Item -ItemType Directory -Force -Path $bin | Out-Null

# Older .NET defaults refuse GitHub's TLS 1.2; setting it is refused in Constrained Language
# Mode, where curl.exe is used anyway.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072 } catch { }
# Windows' own curl.exe and tar.exe (System32), not whatever PATH finds first: Git for Windows'
# GNU tar reads `C:\…` as a remote host ("Cannot connect to C: resolve failed").
$system32 = Join-Path $env:SystemRoot 'System32'
$curl = Get-Item -LiteralPath (Join-Path $system32 'curl.exe') -ErrorAction SilentlyContinue

# No release is published yet (or not that version): say what to do instead.
function NoRelease([string]$Url) {
    "no MagicPic release at $Url (HTTP 404): none is published yet, or not that version. Build from source instead: https://github.com/$Repo/blob/main/docs/agent-setup-source.md"
}

function Fetch([string]$Name) {
    $target = Join-Path $work $Name
    if ($From) {
        $source = Join-Path $From $Name
        if (-not (Test-Path $source)) { Fail "$source is missing" }
        Copy-Item -Force $source $target
        return $target
    }
    if ($Version -eq 'latest') { $url = "https://github.com/$Repo/releases/latest/download/$Name" }
    else { $url = "https://github.com/$Repo/releases/download/$Version/$Name" }
    Say "downloading $url"
    if ($curl) {
        $code = & $curl.FullName -sSL --retry 3 -w '%{http_code}' -o $target $url
        if ($code -eq '404') { Fail (NoRelease $url) }
        if ($LASTEXITCODE -ne 0 -or $code -notmatch '^2') { Fail "download failed: $url (HTTP $code)" }
    } else {
        try { Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $target } catch {
            if ("$_" -match '404') { Fail (NoRelease $url) }
            Fail "download failed: $url ($_)"
        }
    }
    return $target
}

$sums = Fetch 'SHA256SUMS'
$zip = Fetch $archive

$expected = ''
foreach ($line in (Get-Content $sums)) {
    $parts = $line -split '\s+', 2
    if ($parts.Count -eq 2 -and ($parts[1].TrimStart('*') -eq $archive)) { $expected = $parts[0].ToLower() }
}
if (-not $expected) { Fail "SHA256SUMS has no line for $archive" }
$actual = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLower()
if ($actual -ne $expected) { Fail "checksum mismatch for $archive (expected $expected, got $actual): nothing was installed" }
Say "checksum ok ($actual)"

$unpacked = Join-Path $work 'x'
New-Item -ItemType Directory -Force -Path $unpacked | Out-Null
$tar = Get-Item -LiteralPath (Join-Path $system32 'tar.exe') -ErrorAction SilentlyContinue
$unpackedOk = $false
if ($tar) {
    & $tar.FullName -xf $zip -C $unpacked
    $unpackedOk = ($LASTEXITCODE -eq 0)
}
if (-not $unpackedOk) {
    try { Expand-Archive -Force -Path $zip -DestinationPath $unpacked; $unpackedOk = $true } catch { }
}
if (-not $unpackedOk) { Fail "could not unpack $archive" }

$names = @('magicpic.exe', 'magicpic-cli.exe')
foreach ($name in $names) {
    if (-not (Test-Path (Join-Path $unpacked $name))) { Fail "$archive has no $name" }
}

# Old copies an earlier run moved aside because they were running then.
Get-ChildItem -LiteralPath $bin -Filter '*.old-*.exe' -ErrorAction SilentlyContinue | ForEach-Object {
    Remove-Item -Force -LiteralPath $_.FullName -ErrorAction SilentlyContinue
}

# A running app is asked to quit, so it restarts on the new version. The new magicpic-cli asks it
# over the control channel and refuses while a document has unsaved changes: then nothing is
# installed. An app it can't ask keeps running the old version until it restarts.
$appExe = Join-Path $bin 'magicpic.exe'
$appRunning = @(Get-Process -Name 'magicpic' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $appExe })
if ($appRunning.Count -gt 0) {
    $quit = $null
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $text = & (Join-Path $unpacked 'magicpic-cli.exe') quit-app 2>$null | Out-String
        $quit = $text | ConvertFrom-Json
    } catch { }
    $ErrorActionPreference = $previous
    if ($quit -and $quit.unsaved) {
        Fail "MagicPic has unsaved changes in $($quit.unsaved -join ', '): nothing was installed. Save or close them in MagicPic, then run the installer again"
    }
    if ($quit -and $quit.stopped) { Say 'asked MagicPic to quit; it starts again on the new version' }
    else { Say 'MagicPic is running and could not be asked to quit: it keeps the old version until it restarts' }
}

# Copy both in beside first, so a failure here changes nothing.
$staged = @()
foreach ($name in $names) {
    $temp = Join-Path $bin ".$name.new"
    try {
        Copy-Item -Force -LiteralPath (Join-Path $unpacked $name) -Destination $temp
    } catch {
        foreach ($t in $staged) { Remove-Item -Force -LiteralPath $t -ErrorAction SilentlyContinue }
        Remove-Item -Force -LiteralPath $temp -ErrorAction SilentlyContinue
        Fail "could not copy $name into ${bin}: $_. Nothing was replaced"
    }
    $staged += $temp
}

# Then swap. A running program (the app, or magicpic-cli.exe that an agent client keeps running as
# its MCP servers) can be renamed but not overwritten, so the old file moves aside as
# <name>.old-<time>.exe and is deleted now if it isn't running, else on the next run.
$stamp = Get-Date -Format 'yyyyMMddTHHmmss'
$doneTargets = @()
$doneOld = @()
function Undo {
    for ($i = $doneTargets.Count - 1; $i -ge 0; $i--) {
        if ($doneOld[$i]) { Move-Item -Force -LiteralPath $doneOld[$i] -Destination $doneTargets[$i] -ErrorAction SilentlyContinue }
    }
    foreach ($t in $staged) { Remove-Item -Force -LiteralPath $t -ErrorAction SilentlyContinue }
}
$inUse = @()
foreach ($name in $names) {
    $target = Join-Path $bin $name
    $temp = Join-Path $bin ".$name.new"
    $old = ''
    if (Test-Path -LiteralPath $target) {
        $old = Join-Path $bin ($name -replace '\.exe$', ".old-$stamp.exe")
        try { Move-Item -Force -LiteralPath $target -Destination $old } catch {
            Undo
            Fail "could not move $target aside ($_): nothing was replaced. Quit MagicPic and the agent client using it, then run the installer again"
        }
    }
    try { Move-Item -Force -LiteralPath $temp -Destination $target } catch {
        if ($old) { Move-Item -Force -LiteralPath $old -Destination $target -ErrorAction SilentlyContinue }
        Undo
        Fail "could not put the new $name in place ($_): nothing was replaced"
    }
    $doneTargets += $target
    $doneOld += $old
    # A file downloaded by a browser carries the internet zone; ours shouldn't, but be sure.
    Unblock-File -Path $target -ErrorAction SilentlyContinue
}
for ($i = 0; $i -lt $doneOld.Count; $i++) {
    if ($doneOld[$i]) {
        Remove-Item -Force -LiteralPath $doneOld[$i] -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $doneOld[$i]) { $inUse += $doneOld[$i] }
    }
}
if ($inUse.Count -gt 0) {
    Say "an old copy is still running (an agent client's MCP servers): $($inUse -join ', '). It is deleted on a later run; restart that client to load the new servers"
}
foreach ($name in @('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')) {
    $source = Join-Path $unpacked $name
    if (Test-Path $source) { Copy-Item -Force $source (Join-Path $home_ $name) }
}
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
$cli = Join-Path $bin 'magicpic-cli.exe'
$app = Join-Path $bin 'magicpic.exe'
Say "installed $app and $cli"

if ($NoSetup) {
    @{ ok = $true; bin = $bin; cli = $cli; app = $app } | ConvertTo-Json -Compress
    return
}

$setupArgs = @('setup', '--app', $app)
if ($Photos) { $setupArgs += @('--photos', $Photos) }
if ($Port) { $setupArgs += @('--port', $Port) }
if ($DesktopShortcut) { $setupArgs += '--desktop-shortcut' }
$env:MAGICPIC_HOME = $home_
try {
    & $cli @setupArgs
} catch {
    Fail "could not run $cli ($_). On Windows 11, Smart App Control or an application control policy may block unsigned programs: see Troubleshooting in docs/agent-setup.md"
}
if ($LASTEXITCODE -ne 0) {
    Fail "magicpic-cli setup failed (exit $LASTEXITCODE). If Windows said the file is blocked, see Troubleshooting in docs/agent-setup.md"
}
