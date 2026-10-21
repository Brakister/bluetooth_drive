# Fix-OBSAudio.ps1
#
# Corrige a configuracao do OBS para que ele NAO abra o microfone Bluetooth
# (endpoint HFP "Hands-Free AG Audio") do fone de ouvido.
#
# POR QUE ISSO QUEBRA O BLUETOOTH
# -------------------------------
# O OBS abre a fonte "Mic/Aux" (wasapi_input_capture) com device_id = "default".
# Quando um fone Bluetooth esta conectado, o Windows promove o endpoint HFP do
# fone (16 kHz, mono) a microfone padrao. Ao abrir esse endpoint, o fone troca
# o perfil de A2DP para HFP e o dongle Bluetooth USB (CSR Nanosira clone)
# reinicia o controlador -> a chave de link (que vivia na RAM do dongle) e
# perdida -> BTHUSB evento 16 (falha de autenticacao mutua) -> o fone fica
# "desconectado" e so volta apos reiniciar o PC.
#
# Este script aponta a fonte Mic/Aux para um endpoint de captura NAO-Bluetooth
# (por padrao, o microfone da webcam), eliminando o gatilho.
#
# Uso:  powershell -ExecutionPolicy Bypass -File Fix-OBSAudio.ps1
#        powershell -ExecutionPolicy Bypass -File Fix-OBSAudio.ps1 -DeviceId "{0.0.1.00000000}.{...}"
#        powershell -ExecutionPolicy Bypass -File Fix-OBSAudio.ps1 -Undo

[CmdletBinding()]
param(
    # Endpoint WASAPI a usar como microfone do OBS. Vazio = deteccao automatica.
    [string] $DeviceId = "",
    # Desfaz a alteration (restaurar o backup).
    [switch] $Undo
)

$ErrorActionPreference = "Stop"

$ObsRoot = Join-Path $env:APPDATA "obs-studio\basic"
$BackupDir = Join-Path $ObsRoot "btguard-backup"

function Write-Ok  ($m) { Write-Host "[ok]   $m" -ForegroundColor Green }
function Write-Info($m) { Write-Host "[info] $m" -ForegroundColor Cyan }
function Write-Warn2($m) { Write-Host "[aviso] $m" -ForegroundColor Yellow }

# ---------------------------------------------------------------------------
# Descobre o endpoint de captura NAO-Bluetooth ativo
# ---------------------------------------------------------------------------
function Get-NonBluetoothCapture {
    # Percorre os endpoints de audio registrados no MMDevices e devolve
    #   @{ Id = <id WASAPI>; Guid = <guid>; Name = <nome amigavel> }
    $base = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture"
    $FNAME = "{b3f8fa53-0004-438e-9003-51a46e139bfc}"
    $out = @()
    foreach ($k in Get-ChildItem $base -ErrorAction SilentlyContinue) {
        $state = (Get-ItemProperty $k.PSPath -ErrorAction SilentlyContinue).DeviceState
        if ($state -ne 1) { continue }   # 1 = ACTIVE
        $props = Get-ItemProperty "$($k.PSPath)\Properties" -ErrorAction SilentlyContinue
        $name = $props."$FNAME,6"
        $path = $props."$FNAME,2"
        $blob = "$name $path"
        if ($blob -match "(?i)bthenum|bth|bluetooth|hands-free|headset") { continue }  # e Bluetooth
        $guid = $k.PSChildName.Trim('{','}')
        $out += [pscustomobject]@{
            Guid = $guid
            Name = $name
            Id   = "{0.0.1.00000000}.{$guid}"      # formato de id que o OBS usa
        }
    }
    return $out
}

function Get-DefaultCaptureDeviceId {
    # Le o id do endpoint de captura padrao direto do enumerador WASAPI.
    $src = @'
using System;
using System.Runtime.InteropServices;
public static class DefCap {
  [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"),
   InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDeviceEnumerator {
    int EnumAudioEndpoints(int f, int s, out IntPtr c);
    int GetDefaultAudioEndpoint(int f, int role, out IMMDevice d);
    int GetDevice([MarshalAs(UnmanagedType.LPWStr)] string id, out IMMDevice d);
    int RegisterEndpointNotificationCallback(IntPtr c);
    int UnregisterEndpointNotificationCallback(IntPtr c);
  }
  [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"),
   InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IMMDevice {
    int Activate(ref Guid iid, int ctx, IntPtr p, [MarshalAs(UnmanagedType.IUnknown)] out object o);
    int OpenPropertyStore(int a, out IntPtr p);
    int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
    int GetState(out int s);
  }
  public static string Get() {
    try {
      var cls = new Guid("BCDE0395-E52F-467C-8E3D-C4579291692E");
      var en = (IMMDeviceEnumerator)Activator.CreateInstance(Type.GetTypeFromCLSID(cls));
      IMMDevice d;
      if (en.GetDefaultAudioEndpoint(1, 1, out d) != 0) return null;   // eCapture, eConsole
      string id; int st;
      if (d.GetId(out id) != 0 || d.GetState(out st) != 0) return null;
      return id;
    } catch { return null; }
  }
}
'@
    try {
        Add-Type -TypeDefinition $src -ErrorAction Stop
        return [DefCap]::Get()
    } catch { return $null }
}

# ---------------------------------------------------------------------------
# Desfazer
# ---------------------------------------------------------------------------
if ($Undo) {
    if (Test-Path $BackupDir) {
        Get-ChildItem $BackupDir -Filter *.json | ForEach-Object {
            $dest = Join-Path $ObsRoot "scenes\$($_.Name)"
            Copy-Item $_.FullName $dest -Force
            Write-Ok "restaurado $dest"
        }
        Write-Info "Backup em $BackupDir (nao removido)."
    } else {
        Write-Warn2 "Nenhum backup encontrado em $BackupDir"
    }
    exit 0
}

# ---------------------------------------------------------------------------
# Escolha do endpoint
# ---------------------------------------------------------------------------
if (-not $DeviceId) {
    $cands = Get-NonBluetoothCapture
    if ($cands.Count -eq 0) {
        Write-Warn2 "Nenhum endpoint de captura NAO-Bluetooth ativo encontrado."
        Write-Warn2 "Usando a alternativa: desabilitar a fonte Mic/Aux do OBS."
        $DeviceId = ""
    } else {
        $default = Get-DefaultCaptureDeviceId
        # Prefere um endpoint que ja seja o padrao; senao o primeiro disponivel.
        $pick = $cands | Where-Object { $_.Id -eq $default } | Select-Object -First 1
        if (-not $pick) { $pick = $cands[0] }
        $DeviceId = $pick.Id
        Write-Info "Endpoint de captura escolhido: $($pick.Name)"
        Write-Info "  id: $DeviceId"
    }
}

# ---------------------------------------------------------------------------
# Aplicacao
# ---------------------------------------------------------------------------
$scenesDir = Join-Path $ObsRoot "scenes"
if (-not (Test-Path $scenesDir)) {
    Write-Warn2 "OBS nao encontrado em $ObsRoot - nada a fazer."
    exit 0
}

$changed = 0
foreach ($f in Get-ChildItem $scenesDir -Filter *.json -ErrorAction SilentlyContinue) {
    # UTF-8 explicito: o PowerShell 5.1 leria como ANSI e corromperia os acentos.
    $raw = [System.IO.File]::ReadAllText($f.FullName, [System.Text.Encoding]::UTF8)
    try { $j = $raw | ConvertFrom-Json } catch { Write-Warn2 "JSON invalido em $($f.Name)"; continue }

    if (-not $j.PSObject.Properties.Name.Contains("AuxAudioDevice1")) { continue }
    $aux = $j.AuxAudioDevice1
    if (-not $aux.settings) { continue }

    $old = $aux.settings.device_id
    if ($old -eq $DeviceId) { Write-Info "$($f.Name): ja esta em '$DeviceId'"; continue }

    # backup
    if (-not (Test-Path $BackupDir)) { New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null }
    Copy-Item $f.FullName (Join-Path $BackupDir $f.Name) -Force

    # Edicao textual-cirurgica: preserva o resto do JSON do OBS intacto.
    $text = $raw
    $start = $text.IndexOf('"AuxAudioDevice1"')
    if ($start -lt 0) { continue }

    if ($DeviceId -eq "") {
        # alternativa: desabilitar a fonte (procura "enabled": true no bloco)
        $m = [regex]::Match($text.Substring($start), '"enabled"\s*:\s*true')
        if ($m.Success) {
            $idx = $start + $m.Index
            $text = $text.Remove($idx, $m.Length).Insert($idx, '"enabled": false')
            Write-Ok "$($f.Name): fonte Mic/Aux DESABILITADA (backup em $BackupDir)"
        } else {
            Write-Warn2 "$($f.Name): nao foi possivel desabilitar a fonte."
            continue
        }
    } else {
        $m = [regex]::Match($text.Substring($start), '"device_id"\s*:\s*"(?<v>[^"]*)"')
        if (-not $m.Success) { Write-Warn2 "$($f.Name): device_id nao encontrado."; continue }
        $idx = $start + $m.Index
        $new = '"device_id": "' + $DeviceId + '"'
        $text = $text.Remove($idx, $m.Length).Insert($idx, $new)
        Write-Ok "$($f.Name): Mic/Aux -> '$DeviceId' (antes: '$old')"
    }
    $changed++
    # UTF-8 sem BOM (o OBS nao gosta de BOM)
    [System.IO.File]::WriteAllText($f.FullName, $text, (New-Object System.Text.UTF8Encoding($false)))
}

if ($changed -eq 0) { Write-Info "Nenhuma alteracao necessaria." }
else { Write-Ok "$changed arquivo(s) de cena alterado(s). Reinicie o OBS." }
