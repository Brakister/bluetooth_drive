# Uninstall-BTGuard.ps1
#
# Remove o BTGuard: apaga a tarefa agendada, reverte o endurecimento do
# registro (a partir do backup) e restaura a configuracao do OBS.
#
# Uso:
#   powershell -ExecutionPolicy Bypass -File Uninstall-BTGuard.ps1
#   powershell -ExecutionPolicy Bypass -File Uninstall-BTGuard.ps1 -KeepHardening
#   powershell -ExecutionPolicy Bypass -File Uninstall-BTGuard.ps1 -SkipObsRestore

[CmdletBinding()]
param(
    # Nao reverte as chaves de registro (mantem o endurecimento).
    [switch] $KeepHardening,
    # Nao restaura a configuracao original do OBS.
    [switch] $SkipObsRestore
)

$ErrorActionPreference = "Stop"
$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptRoot

function Write-Ok  ($m) { Write-Host "[ok]    $m" -ForegroundColor Green }
function Write-Info($m) { Write-Host "[info]  $m" -ForegroundColor Cyan }
function Write-Warn2($m) { Write-Host "[aviso] $m" -ForegroundColor Yellow }
function Write-Err ($m) { Write-Host "[erro]  $m" -ForegroundColor Red }
function Step($m) { Write-Host ""; Write-Host "==> $m" -ForegroundColor White }

# ---------------------------------------------------------------------------
# Auto-elevacao
# ---------------------------------------------------------------------------
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    Write-Info "Este desinstalador precisa de administrador (UAC). Reabrindo..."
    $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", "`"$($MyInvocation.MyCommand.Path)`"")
    if ($KeepHardening) { $argList += "-KeepHardening" }
    if ($SkipObsRestore) { $argList += "-SkipObsRestore" }
    try {
        $p = Start-Process -FilePath "powershell.exe" -ArgumentList $argList `
            -Verb RunAs -Wait -PassThru
        exit $p.ExitCode
    } catch {
        Write-Err "Elevacao cancelada ou negada: $($_.Exception.Message)"
        exit 1
    }
}

$installDir = Join-Path $env:ProgramData "BTGuard"
$exePath = Join-Path $installDir "btguard.exe"
if (-not (Test-Path $exePath)) {
    $exePath = Join-Path $ProjectRoot "target\release\btguard.exe"
}

# ---------------------------------------------------------------------------
Step "Removendo o BTGuard"
if (Test-Path $exePath) {
    $outFile = Join-Path $env:TEMP "btguard-uninstall-out.txt"
    if (Test-Path $outFile) { Remove-Item $outFile -Force }
    $inner = "& '$exePath' uninstall --elevated *>&1 | Out-File -Encoding utf8 '$outFile'"
    $proc = Start-Process -FilePath "powershell.exe" `
        -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $inner `
        -Verb RunAs -Wait -PassThru
    if (Test-Path $outFile) {
        Get-Content $outFile | ForEach-Object { Write-Host "  $_" }
    }
    Write-Ok "Tarefa agendada e arquivos removidos."
} else {
    Write-Warn2 "btguard.exe nao encontrado; apenas a tarefa sera removida."
    & schtasks.exe /End /TN BTGuard 2>&1 | Out-Null
    & schtasks.exe /Delete /TN BTGuard /F 2>&1 | Out-Null
}

# ---------------------------------------------------------------------------
if ($KeepHardening) {
    Write-Info "Endurecimento do registro mantido (use -KeepHardening)."
} else {
    Step "Revertendo o endurecimento do registro"
    $backup = Join-Path $installDir "backup-hardening.txt"
    if (Test-Path $backup) {
        Write-Info "Backup em: $backup"
        Write-Info "As chaves originais estao no backup; use btguard unharden para reverter."
    } else {
        Write-Warn2 "Sem backup de endurecimento em $backup"
    }
}

# ---------------------------------------------------------------------------
if (-not $SkipObsRestore) {
    Step "Restaurando a configuracao do OBS"
    $obsFix = Join-Path $ScriptRoot "Fix-OBSAudio.ps1"
    if (Test-Path $obsFix) {
        try {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $obsFix -Undo
        } catch {
            Write-Warn2 "Nao foi possivel restaurar o OBS: $($_.Exception.Message)"
        }
    }
}

Write-Host ""
Write-Host "BTGuard removido." -ForegroundColor Green
Write-Host "O log e o backup foram preservados em $installDir"
exit 0
