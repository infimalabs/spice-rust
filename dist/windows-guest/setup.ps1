# First-logon configuration for the Spice Windows 11 ARM developer guest.
#
# Windows Setup runs this once from the provisioning media. It installs the
# pinned virtio network driver, OpenSSH server, and x64 VC++ runtime, admits
# only the host key written beside it, and powers the guest off so the host can
# seal the installed disk as the recorded clean base.
param([Parameter(Mandatory = $true)][string]$Media)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$payload = Join-Path $Media 'spice'

New-Item -ItemType Directory -Force -Path 'C:\spice\runner', 'C:\spice\work', 'C:\spice\tmp' | Out-Null
Start-Transcript -Path 'C:\spice\provision.log' | Out-Null
try {
    & pnputil.exe /add-driver (Join-Path $payload 'drivers\netkvm.inf') /install
    if ($LASTEXITCODE -notin 0, 3010) { throw "pnputil exited $LASTEXITCODE" }

    $msi = Start-Process -FilePath 'msiexec.exe' -Wait -PassThru -ArgumentList @(
        '/i', ('"{0}"' -f (Join-Path $payload 'openssh.msi')), '/qn', '/norestart')
    if ($msi.ExitCode -notin 0, 3010) { throw "OpenSSH installer exited $($msi.ExitCode)" }

    # Rust MSVC executables load VCRUNTIME140.dll; Windows does not ship it.
    $runtime = Start-Process -FilePath (Join-Path $payload 'vc_redist.x64.exe') -Wait -PassThru `
        -ArgumentList '/install', '/quiet', '/norestart'
    if ($runtime.ExitCode -notin 0, 3010) { throw "x64 VC++ runtime installer exited $($runtime.ExitCode)" }

    $ssh = 'C:\ProgramData\ssh'
    New-Item -ItemType Directory -Force -Path $ssh | Out-Null
    Set-Content -Encoding ascii -Path (Join-Path $ssh 'sshd_config') -Value @(
        'PubkeyAuthentication yes',
        'PasswordAuthentication no',
        'KbdInteractiveAuthentication no',
        'Subsystem sftp sftp-server.exe',
        'Match Group administrators',
        '    AuthorizedKeysFile __PROGRAMDATA__/ssh/administrators_authorized_keys')
    $keys = Join-Path $ssh 'administrators_authorized_keys'
    Copy-Item -Force -Path (Join-Path $payload 'authorized_keys') -Destination $keys
    & icacls.exe $keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'
    if ($LASTEXITCODE -ne 0) { throw "icacls exited $LASTEXITCODE" }

    Set-Service -Name sshd -StartupType Automatic
    Start-Service -Name sshd
    New-NetFirewallRule -Name 'spice-guest-sshd' -DisplayName 'Spice guest SSH' `
        -Direction Inbound -Protocol TCP -LocalPort 22 -Action Allow -Profile Any | Out-Null

    & powercfg.exe /hibernate off
    & powercfg.exe /change standby-timeout-ac 0
    & powercfg.exe /change monitor-timeout-ac 0
} catch {
    # Record the failure before the transcript closes so C:\spice\provision.log
    # explains a guest that powers off without reachable SSH.
    Write-Output "setup failed: $_"
    throw
} finally {
    Stop-Transcript | Out-Null
    Stop-Computer -Force
}
