// Pareamento de dispositivos Bluetooth pela API do Windows.
//
// Usa exatamente o caminho que o Settings usa por tras
// (bluetoothapis.dll + bthprops.cpl), sem depender de automacao de UI:
//
//   BluetoothFindFirstRadio        -> handle do radio
//   BluetoothFindFirstDevice       -> varredura RF (fIssueInquiry=1) = "modo
//                                     pareamento" (so encontra quem esta
//                                     anunciando-se para parear)
//   BluetoothAuthenticateDevice    -> pareamento SSP (abre o popup do
//                                     proprio Windows para o usuario confirmar)
//   BluetoothSetServiceState       -> habilita os servicos de audio (A2DP/HFP)
//                                     sem os quais o fone nao aparece como
//                                     dispositivo de saida/entrada de audio
//   BluetoothRemoveDevice          -> limpa um pareamento antigo/broken
//
// Nada aqui e polling de sistema: sao chamadas pontuais do usuario.

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Foundation::HANDLE;

use crate::util::from_wide;
use crate::util::log;

/// Servicos que precisam estar habilitados no fone para ele virar o
/// dispositivo de audio do Windows (saida A2DP + microfone HFP).
///
/// UUID no formato {0000XXXX-0000-1000-8000-00805F9B34FB}.
pub const AUDIO_SERVICES: [(&str, GUID); 8] = [
    ("Audio Sink (A2DP)", GUID::from_u128(0x0000110b00001000800000805f9b34fb)),
    (
        "Advanced Audio Distribution",
        GUID::from_u128(0x0000110d00001000800000805f9b34fb),
    ),
    ("Generic Audio", GUID::from_u128(0x0000120300001000800000805f9b34fb)),
    ("Handsfree (HFP)", GUID::from_u128(0x0000111e00001000800000805f9b34fb)),
    ("Handsfree AG", GUID::from_u128(0x0000111f00001000800000805f9b34fb)),
    (
        "Audio Remote Control",
        GUID::from_u128(0x0000110f00001000800000805f9b34fb),
    ),
    ("Message Access", GUID::from_u128(0x0000113200001000800000805f9b34fb)),
    ("Object Push", GUID::from_u128(0x0000110500001000800000805f9b34fb)),
];

/// Um dispositivo visto na varredura RF.
#[derive(Debug, Clone)]
pub struct Found {
    /// Endereco MAC em hex minusculo, sem separadores.
    pub addr: String,
    pub name: String,
    pub cod: u32,
    pub connected: bool,
    pub remembered: bool,
}

/// Formata um GUID no estilo do registro do Windows:
/// {0000110b-0000-1000-8000-00805f9b34fb}
pub fn guid_str(g: &GUID) -> String {
    let d = &g.data4;
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// Converte "ee:e7:a9:aa:a8:af" (ou "eee7a9aaa8af") em BLUETOOTH_ADDRESS.
///
/// Atencao: em `rgBytes` o byte MENOS significativo vem primeiro, ou seja, a
/// ordem e invertida em relacao ao MAC que aparece na interface do Windows.
fn parse_addr(s: &str) -> Option<BLUETOOTH_ADDRESS> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let mut b = [0u8; 6];
    for (i, slot) in b.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    let mut a: BLUETOOTH_ADDRESS = unsafe { std::mem::zeroed() };
    a.Anonymous.rgBytes = [b[5], b[4], b[3], b[2], b[1], b[0]];
    Some(a)
}

/// Enumera os radios Bluetooth. O handle fica valido enquanto o processo
/// existir (a API nao exige Close explícito).
pub fn radios() -> Vec<HANDLE> {
    unsafe {
        let mut frp: BLUETOOTH_FIND_RADIO_PARAMS = std::mem::zeroed();
        frp.dwSize = std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
        let mut h: HANDLE = std::ptr::null_mut();
        let find = BluetoothFindFirstRadio(&frp, &mut h);
        if find.is_null() {
            return Vec::new();
        }
        let mut out = vec![h];
        loop {
            h = std::ptr::null_mut();
            if BluetoothFindNextRadio(find, &mut h) == 0 {
                break;
            }
            out.push(h);
            if out.len() > 16 {
                break;
            }
        }
        BluetoothFindRadioClose(find);
        out
    }
}

/// Varredura RF de verdade (modo pareamento). `timeout_mult` 8 ~ 10 s.
pub fn discover(radio: HANDLE, timeout_mult: u8) -> Vec<Found> {
    unsafe {
        let mut sp: BLUETOOTH_DEVICE_SEARCH_PARAMS = std::mem::zeroed();
        sp.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        sp.fReturnAuthenticated = 1;
        sp.fReturnRemembered = 1;
        sp.fReturnUnknown = 1;
        sp.fReturnConnected = 1;
        sp.fIssueInquiry = 1;
        sp.cTimeoutMultiplier = timeout_mult;
        sp.hRadio = radio;

        let mut info: BLUETOOTH_DEVICE_INFO = std::mem::zeroed();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

        let mut out = Vec::new();
        let h = BluetoothFindFirstDevice(&sp, &mut info);
        if h.is_null() {
            return out;
        }
        loop {
            out.push(found_of(&info));
            info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(h, &mut info) == 0 {
                break;
            }
            if out.len() > 128 {
                break;
            }
        }
        BluetoothFindDeviceClose(h);
        out
    }
}

fn found_of(info: &BLUETOOTH_DEVICE_INFO) -> Found {
    let bytes = unsafe { info.Address.Anonymous.rgBytes };
    let addr: String = bytes.iter().rev().map(|b| format!("{b:02x}")).collect();
    Found {
        addr,
        name: from_wide(&info.szName),
        cod: info.ulClassofDevice,
        connected: info.fConnected != 0,
        remembered: info.fRemembered != 0,
    }
}

/// Remove o pareamento (despareia). Usado para limpar uma chave de link
/// quebrada antes de refazer o pareamento do zero.
pub fn remove(addr: &str) -> Result<(), String> {
    let a = parse_addr(addr).ok_or_else(|| format!("endereco invalido: {addr}"))?;
    unsafe {
        let r = BluetoothRemoveDevice(&a);
        if r == 0 {
            Ok(())
        } else {
            Err(format!("BluetoothRemoveDevice falhou: {r}"))
        }
    }
}

/// Executa o pareamento SSP de um dispositivo ja descoberto.
///
/// O proprio Windows mostra o popup de confirmacao ("Emparelhar dispositivo?"),
/// igual ao Settings. Passkey NULL = SSP sem PIN (Just Works / Secure Simple
/// Pairing, que e o caso dos fones BLE/EDR modernos).
pub fn authenticate(radio: HANDLE, addr: &str) -> Result<(), String> {
    let a = parse_addr(addr).ok_or_else(|| format!("endereco invalido: {addr}"))?;
    unsafe {
        let mut info: BLUETOOTH_DEVICE_INFO = std::mem::zeroed();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        info.Address = a;
        let r = BluetoothAuthenticateDevice(
            std::ptr::null_mut(),
            radio,
            &mut info,
            std::ptr::null(),
            0,
        );
        if r == 0 {
            Ok(())
        } else {
            Err(format!("BluetoothAuthenticateDevice falhou: codigo {r}"))
        }
    }
}

/// Habilita os servicos de audio no dispositivo pareado.
///
/// Sem isso o fone fica pareado mas nao aparece como saida de audio nem
/// como microfone (que e exatamente o bug do OBS apontado no relatorio).
pub fn enable_services(radio: HANDLE, addr: &str) -> Vec<(String, Result<(), u32>)> {
    let a = match parse_addr(addr) {
        Some(v) => v,
        None => return Vec::new(),
    };
    unsafe {
        let mut info: BLUETOOTH_DEVICE_INFO = std::mem::zeroed();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        info.Address = a;
        let mut out = Vec::new();
        for (name, guid) in AUDIO_SERVICES {
            // BLUETOOTH_SERVICE_ENABLE = 1. Usar 0 DESLIGARIA o servico.
            let r = BluetoothSetServiceState(radio, &mut info, &guid, BLUETOOTH_SERVICE_ENABLE);
            out.push((name.to_string(), if r == 0 { Ok(()) } else { Err(r) }));
        }
        out
    }
}

/// Re-pareia um dispositivo ja conhecido.
///
/// Usado pelo vigia quando o pareamento fica "velho": o host guarda a chave
/// de link apenas na RAM (nao existe nenhum valor de chave de link no
/// registro - ver RELATORIO-BT.md), entao um reinicio do PC ou uma
/// corrupcao faz o pareamento virar "lembrado mas sem chave" e a
/// autenticacao falha (BTHUSB id 16) ate o dispositivo ser pareado de novo.
///
/// Sequencia: remove o registro quebrado -> procura o dispositivo em modo
/// pareamento -> emparelha -> religa os servicos de audio.
pub fn re_pair(addr: &str, timeout_s: u64) -> Result<(), String> {
    let radios = radios();
    let radio = *radios.first().ok_or("nenhum radio Bluetooth")?;

    log("INFO", &format!("re-pareando {addr}: limpando o pareamento antigo"));
    let _ = remove(addr);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    loop {
        for d in discover(radio, 4) {
            let alvo = d.addr.eq_ignore_ascii_case(addr);
            if alvo {
                log("INFO", &format!("pareando {} ({})", d.name, d.addr));
                authenticate(radio, &d.addr)?;
                for (name, r) in enable_services(radio, &d.addr) {
                    match r {
                        Ok(()) => log("INFO", &format!("servico ativado: {name}")),
                        Err(c) => log("INFO", &format!("servico indisponivel: {name} ({c})")),
                    }
                }
                log("INFO", &format!("re-pareamento de {addr} concluido"));
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("{addr} nao apareceu na varredura de pareamento"));
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}

/// Estado (servicos habilitados) do dispositivo.
///
/// ATENCAO: nao existe API de consulta de estado de servico remoto na
/// pilha do Windows. `BluetoothSetServiceState` com `BLUETOOTH_SERVICE_DISABLE`
/// (0) DESLIGA o servico - usar "0" como se fosse consulta desliga o audio
/// silenciosamente (bug observado em 21/10/2026: o A2DP caia em ~3 s).
/// Por isso o estado e lido do registro, onde o Windows espelha o que esta
/// realmente ativo.
pub fn service_state(addr: &str) -> Vec<(String, bool)> {
    use crate::reg;
    let base = format!(
        r"{}\{}\ServicesFor001a7dda7113",
        crate::pnp::BTHPORT_DEVICES_KEY,
        addr.to_lowercase()
    );
    let mut out = Vec::new();
    for (name, guid) in AUDIO_SERVICES {
        let enabled = reg::open_read(&format!("{base}\\{}", guid_str(&guid)))
            .ok()
            .and_then(|k| reg::read_dword(&k, "Enabled").ok().flatten())
            .map(|v| v != 0)
            .unwrap_or(false);
        out.push((name.to_string(), enabled));
    }
    out
}
