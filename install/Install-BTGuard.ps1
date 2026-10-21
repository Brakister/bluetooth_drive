# Install-BTGuard.ps1
#
# Instalador do BTGuard: supervisor do radio Bluetooth USB (CSR Nanosira) que
# impede que o OBS (ou qualquer app que abra o microfone HFP) derrube o
# pareamento do fone de ouvido.
#
# O que ele faz:
#   1. Compila (ou localiza) o btguard.exe
#   2. Roda `btguard install --elevated`:
#        - copia o executavel para %ProgramData%\BTGuard
#        - aplica o endurecimento do driver (registro, com backup)
#        - cria a tarefa agendada "BTGuard" (boot +30s, SYSTEM, prioridade 7)
#        - inicia a vigilancia imediatamente
#   3. Roda Fix-OBSAudio.ps1, que aponta a fonte Mic/Aux do OBS para um
#      microfone NAO-Bluetooth (o gatilho original)
#
# Uso:
#   powershell -ExecutionPolicy Bypass -File Install-BTGuard.ps1
#   powershell -ExecutionPolicy Bypass -File Install-BTGuard.ps1 -SkipObsFix
#   powershell -ExecutionPolicy Bypass -File Install-BTGuard.ps1 -Exe C:\caminho\btguard.exe

[CmdletBinding()]
param(
    # Binario ja compilado. Vazio = compilar a partir do fonte com cargo.
    [string] $Exe = "",
    # Nao mexe na configuracao do OBS.
    [switch] $SkipObsFix,
    # Nao abre a janela do powershell elevada (usado na auto-elevacao).
    [switch] $NoPause
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
    Write-Info "Este instalador precisa de administrador (UAC). Reabrindo..."
    $argList = @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", "`"$($MyInvocation.MyCommand.Path)`"",
        "-Exe", "`"$Exe`""
    )
    if ($SkipObsFix) { $argList += "-SkipObsFix" }
    try {
        $p = Start-Process -FilePath "powershell.exe" -ArgumentList $argList `
            -Verb RunAs -Wait -PassThru
        exit $p.ExitCode
    } catch {
        Write-Err "Elevacao cancelada ou negada: $($_.Exception.Message)"
        exit 1
    }
}

Step "Verificando o ambiente"
$installDir = Join-Path $env:ProgramData "BTGuard"
Write-Info "Diretorio de instalacao: $installDir"

# ---------------------------------------------------------------------------
# 1) Localiza / compila o executavel
# ---------------------------------------------------------------------------
Step "Preparando o btguard.exe"

$exePath = $Exe
if ([string]::IsNullOrWhiteSpace($exePath)) {
    $built = Join-Path $ProjectRoot "target\release\btguard.exe"
    if (Test-Path $built) {
        $exePath = $built
        Write-Info "Usando o binario ja compilado: $exePath"
    } else {
        $cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
        if (-not (Test-Path $cargo)) {
            $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
        }
        if (-not $cargo -or -not (Test-Path $cargo)) {
            Write-Err "cargo nao encontrado. Instale o Rust (https://rustup.rs) ou use -Exe <caminho>."
            exit 1
        }
        Write-Info "Compilando (cargo build --release)..."
        Push-Location $ProjectRoot
        & $cargo build --release --manifest-path (Join-Path $ProjectRoot "Cargo.toml")
        $rc = $LASTEXITCODE
        Pop-Location
        if ($rc -ne 0 -or -not (Test-Path $built)) {
            Write-Err "Falha na compilacao."
            exit 1
        }
        $exePath = $built
    }
}

if (-not (Test-Path $exePath)) {
    Write-Err "Executavel nao encontrado: $exePath"
    exit 1
}
Write-Ok "Executavel: $exePath ($((Get-Item $exePath).Length) bytes)"

# ---------------------------------------------------------------------------
# 2) btguard install --elevated
# ---------------------------------------------------------------------------
Step "Instalando o BTGuard (endurecimento + tarefa agendada)"
Write-Info "Voce pode receber um prompt do UAC de novo; aceite."

$outFile = Join-Path $env:TEMP "btguard-install-out.txt"
if (Test-Path $outFile) { Remove-Item $outFile -Force }

# Start-Process -Verb RunAs nao combina com redirecionamento: envelopa em
# Start-Process powershell -Command e grava a saida num arquivo.
$inner = "& '$exePath' install --elevated *>&1 | Out-File -Encoding utf8 '$outFile'"
$proc = Start-Process -FilePath "powershell.exe" `
    -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $inner `
    -Verb RunAs -Wait -PassThru

if (Test-Path $outFile) {
    Get-Content $outFile | ForEach-Object { Write-Host "  $_" }
}
if ($proc.ExitCode -ne 0) {
    Write-Err "btguard install falhou (codigo $($proc.ExitCode))."
    exit $proc.ExitCode
}
Write-Ok "BTGuard instalado e a vigilancia foi iniciada."

# ---------------------------------------------------------------------------
# 3) Verificacao
# ---------------------------------------------------------------------------
Step "Verificando"
$task = (schtasks /Query /TN BTGuard /FO LIST /V 2>&1 | Out-String)
if ($task -match "Em execu|Executing|Running") {
    Write-Ok "Tarefa 'BTGuard' em execucao."
} elseif ($task -match "Acesso negado|Access denied") {
    Write-Info "Tarefa 'BTGuard' criada (consulta de estado exige elevacao)."
} else {
    Write-Warn2 "Nao foi possivel confirmar o estado da tarefa."
}
Write-Ok "Log do vigia: $installDir\btguard.log"

# ---------------------------------------------------------------------------
# 4) Correcao do OBS
# ---------------------------------------------------------------------------
if (-not $SkipObsFix) {
    Step "Corrigindo a captura de audio do OBS"
    $obsFix = Join-Path $ScriptRoot "Fix-OBSAudio.ps1"
    if (Test-Path $obsFix) {
        try {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $obsFix
        } catch {
            Write-Warn2 "Nao foi possivel corrigir o OBS: $($_.Exception.Message)"
        }
    } else {
        Write-Warn2 "Fix-OBSAudio.ps1 nao encontrado em $ScriptRoot"
    }
}

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "==========================================================" -ForegroundColor Green
Write-Host " BTGuard instalado" -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
Write-Host ""
Write-Host "  * O driver foi endurecido (registro salvo em $installDir\backup-hardening.txt)"
Write-Host "  * A tarefa 'BTGuard' roda como SYSTEM a cada boot (+30s)"
Write-Host "  * O vigia reinicia o radio sozinho se ele travar - sem reiniciar o PC"
Write-Host ""
Write-Host "  Comandos uteis:"
Write-Host "    btguard status     - diagnostico completo"
Write-Host "    btguard recover    - reinicia o radio agora"
Write-Host "    btguard uninstall  - remove tudo"
Write-Host ""
if (-not $NoPause) {
    $null = Read-Host "Pressione ENTER para sair"
}
exit 0
