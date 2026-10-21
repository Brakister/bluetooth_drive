// Camada de dispositivos: localizacao do radio USB-BT, estado do devnode e reinicio.

use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;

use crate::reg;
use crate::util::{from_wide, wide};

/// HWID do radio CSR Nanosira generico (VID_0A12 / PID_0001).
pub const RADIO_HWID: &str = r"USB\VID_0A12&PID_0001";
pub const RADIO_ENUM_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Enum\USB\VID_0A12&PID_0001";
pub const BTHPORT_DEVICES_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices";

pub fn radio_instances() -> Vec<String> {
    match reg::open_read(RADIO_ENUM_KEY) {
        Ok(k) => reg::enum_subkeys(&k).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn radio_device_params(instance: &str) -> String {
    format!(r"{RADIO_ENUM_KEY}\{instance}\Device Parameters")
}

/// Localiza o devinst (handle numerico) de um id de instancia completo.
pub fn locate(full_id: &str) -> Option<u32> {
    let mut devinst: u32 = 0;
    let r = unsafe { CM_Locate_DevNodeW(&mut devinst, wide(full_id).as_ptr(), 0) };
    if r == CR_SUCCESS {
        Some(devinst)
    } else {
        None
    }
}

/// (status, problema) do devnode.
pub fn status(devinst: u32) -> Option<(u32, u32)> {
    let mut st: u32 = 0;
    let mut prob: u32 = 0;
    let r = unsafe { CM_Get_DevNode_Status(&mut st, &mut prob, devinst, 0) };
    if r == CR_SUCCESS {
        Some((st, prob))
    } else {
        None
    }
}

pub fn is_started(devinst: u32) -> bool {
    matches!(status(devinst), Some((s, _)) if s & DN_STARTED != 0)
}

/// Radio presente e funcionando (devnode iniciado, sem problema).
pub fn radio_healthy() -> Option<(String, u32)> {
    for inst in radio_instances() {
        let full = format!("{RADIO_HWID}\\{inst}");
        if let Some(di) = locate(&full) {
            if is_started(di) {
                return Some((full, di));
            }
        }
    }
    None
}

/// Qualquer instancia do radio presente no sistema (mesmo nao iniciada).
pub fn radio_any() -> Option<(String, u32)> {
    for inst in radio_instances() {
        let full = format!("{RADIO_HWID}\\{inst}");
        if let Some(di) = locate(&full) {
            return Some((full, di));
        }
    }
    None
}

/// Reinicia o radio: desabilita e reabilita o devnode (forca re-enumeracao USB).
pub fn restart_radio(devinst: u32) -> Result<(), String> {
    let r = unsafe { CM_Disable_DevNode(devinst, 0) };
    if r != CR_SUCCESS {
        return Err(format!("CM_Disable_DevNode falhou: {r}"));
    }
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let r = unsafe { CM_Enable_DevNode(devinst, 0) };
    if r != CR_SUCCESS {
        return Err(format!("CM_Enable_DevNode falhou: {r}"));
    }
    Ok(())
}

/// Espera o radio voltar a estar iniciado.
pub fn wait_started(devinst: u32, timeout_s: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
    while std::time::Instant::now() < deadline {
        if is_started(devinst) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    false
}

/// Dispositivo Bluetooth visto pela pilha do Windows.
#[derive(Debug, Clone)]
pub struct BtDevice {
    /// Endereco MAC em hex minusculo, sem separadores (ex.: eee7a9aaa8af).
    pub addr: String,
    pub name: String,
    pub cod: u32,
    pub connected: bool,
    pub remembered: bool,
    pub authenticated: bool,
}

/// Enumera os dispositivos Bluetooth conhecidos pela pilha do Windows.
///
/// Usa a API oficial (bluetoothapis.dll / BluetoothFindFirstDevice) em vez do
/// SetupAPI: ela devolve o estado REAL de conexao (fConnected), que e o que
/// importa para detectar que o fone se perdeu depois de um reset do radio.
/// `fIssueInquiry = 0` mantem a chamada puramente local (sem varredura RF),
/// portanto e rapida e nao interfere em outros dispositivos.
pub fn bt_devices() -> Vec<BtDevice> {
    unsafe {
        let mut sp: BLUETOOTH_DEVICE_SEARCH_PARAMS = std::mem::zeroed();
        sp.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        sp.fReturnAuthenticated = 1;
        sp.fReturnRemembered = 1;
        sp.fReturnConnected = 1;
        sp.fReturnUnknown = 0;
        sp.fIssueInquiry = 0;
        sp.hRadio = std::ptr::null_mut();

        let mut info: BLUETOOTH_DEVICE_INFO = std::mem::zeroed();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

        let mut out = Vec::new();
        let h = BluetoothFindFirstDevice(&sp, &mut info);
        if h.is_null() {
            return out;
        }
        loop {
            // rgBytes[0] e o byte MENOS significativo do endereco; a forma
            // canonica (como aparece no registro do BTHPORT) e o inverso.
            let bytes = info.Address.Anonymous.rgBytes;
            let addr: String = bytes.iter().rev().map(|b| format!("{b:02x}")).collect::<String>();
            let name = from_wide(&info.szName);
            out.push(BtDevice {
                addr,
                name,
                cod: info.ulClassofDevice,
                connected: info.fConnected != 0,
                remembered: info.fRemembered != 0,
                authenticated: info.fAuthenticated != 0,
            });
            info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(h, &mut info) == 0 {
                break;
            }
            if out.len() > 256 {
                break;
            }
        }
        BluetoothFindDeviceClose(h);
        out
    }
}

/// True se o dispositivo Bluetooth indicado estiver conectado agora.
pub fn bt_device_present(addr: &str) -> bool {
    bt_devices()
        .iter()
        .any(|d| d.addr.eq_ignore_ascii_case(addr) && d.connected)
}

/// Enderecos MAC pareados conhecidos (via registro do BTHPORT).
pub fn paired_devices() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(k) = reg::open_read(BTHPORT_DEVICES_KEY) {
        for name in reg::enum_subkeys(&k).unwrap_or_default() {
            let friendly = reg::open_read(&format!(r"{BTHPORT_DEVICES_KEY}\{name}"))
                .ok()
                .and_then(|d| reg::read_string(&d, "FriendlyName").ok().flatten())
                .unwrap_or_default();
            out.push((name, friendly));
        }
    }
    out
}

