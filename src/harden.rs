// Endurecimento do driver: chaves de registro que o proprio driver BthUsb le.
//
// Descobertas via engenharia reversa (Ghidra/REA) de:
//   C:\Windows\System32\drivers\BTHUSB.SYS  (10.0.26100.8655)
//   C:\Windows\System32\drivers\bthport.SYS (10.0.26100.8655)
//
// 1) BTHUSB!FUN_14001882c  -> le "ForceSelectiveSuspend" e "SelectiveSuspendSupported"
//    no Device Parameters do radio.
//      ForceSelectiveSuspend = 1      => driver FORCA o selective suspend (|0x60 na capability USB)
//      SelectiveSuspendSupported = 0   => driver declara o radio como SEM suporte a suspend
//    Estado observado na maquina: DeviceSelectiveSuspended = 1 (o dongle dorme).
//
// 2) bthport!FUN_1401c522c / FUN_14005bd38 / FUN_1401c5a04
//    "Store Link Key" (REG_DWORD) no registro do dispositivo remoto
//    (BTHPORT\Parameters\Devices\<MAC>) e um MARCADOR de status, nao uma
//    opcao de politica:
//      - FUN_14005bd38 liga o bit 0x1000000 (+0x368) no tratador do evento
//        HCI "Write Link Key Complete", ou seja, so quando o RADIO confirma
//        que gravou a chave de link;
//      - FUN_1401c5a04 persiste "Store Link Key"=1 no registro nesse caso;
//      - FUN_1401c522c le o valor de volta no boot para restaurar o bit.
//    Como este dongle so guarda a chave na RAM (evento BTHUSB id 18: "o
//    Windows nao pode armazenar chaves de link no adaptador local"), o valor
//    e reescrito pela pilha a cada reinicio do radio e nao ha nada a
//    configurar: o BTGuard nao toca nessa chave.

use std::fs;

use crate::pnp;
use crate::reg;
use crate::util::{log, log_dir};

pub const BACKUP_FILE: &str = "backup-hardening.txt";

/// Estado original de um valor (para reversao).
#[derive(Debug, Clone)]
enum Orig {
    Absent,
    Dword(u32),
}

struct Backup {
    lines: Vec<String>,
}

impl Backup {
    fn new() -> Self {
        Backup { lines: Vec::new() }
    }

    fn record(&mut self, scope: &str, name: &str, orig: &Orig) {
        // O backup so vale a primeira vez: se a chave ja foi registrada, o
        // estado original e o de verdade e nao o que acabamos de escrever.
        let prefix = format!("{scope}|{name}|");
        if self.lines.iter().any(|l| l.starts_with(&prefix)) {
            return;
        }
        let line = match orig {
            Orig::Absent => format!("{scope}|{name}|absent"),
            Orig::Dword(v) => format!("{scope}|{name}|dword|{v}"),
        };
        self.lines.push(line);
    }

    fn save(&self) -> Result<(), String> {
        let dir = log_dir().ok_or("sem PROGRAMDATA")?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(BACKUP_FILE);
        fs::write(&path, self.lines.join("\n")).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn load() -> Result<Vec<String>, String> {
        let dir = log_dir().ok_or("sem PROGRAMDATA")?;
        let path = dir.join(BACKUP_FILE);
        let txt = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(txt
            .lines()
            .filter(|l| !l.trim().is_empty())
            // Remove o BOM UTF-8 que o PowerShell 5.1 grava por padrao.
            .map(|l| l.trim_start_matches('\u{feff}').trim().to_string())
            .collect())
    }
}

fn radio_scope(instance: &str) -> String {
    format!("RADIO|{instance}")
}

fn split_scope(scope: &str) -> (String, String) {
    match scope.split_once('|') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (scope.to_string(), String::new()),
    }
}

/// Aplica todo o endurecimento. Retorna um resumo das acoes.
pub fn apply() -> Result<Vec<String>, String> {
    let mut report = Vec::new();
    // Preserva o estado original ja registrado em execucoes anteriores.
    let mut backup = Backup::new();
    backup.lines = Backup::load().unwrap_or_default();

    // ---- 1) Radio: selective suspend -------------------------------------
    let instances = pnp::radio_instances();
    if instances.is_empty() {
        return Err("nenhuma instancia do radio USB\\VID_0A12&PID_0001 encontrada".into());
    }
    for inst in &instances {
        let path = pnp::radio_device_params(inst);
        let key = match reg::open_write(&path) {
            Ok(k) => k,
            Err(e) => {
                report.push(format!("[radio {inst}] NAO PUDOU ABRIR: {e}"));
                continue;
            }
        };
        // selective suspend desligado no driver
        let cur = reg::read_dword(&key, "SelectiveSuspendSupported").unwrap_or(None);
        backup.record(&radio_scope(inst), "SelectiveSuspendSupported", &cur.map_or(Orig::Absent, Orig::Dword));
        reg::write_dword(&key, "SelectiveSuspendSupported", 0)
            .map_err(|e| format!("SelectiveSuspendSupported: {e}"))?;
        report.push(format!("[radio {inst}] SelectiveSuspendSupported=0"));

        // nunca forcar o suspend
        let cur = reg::read_dword(&key, "ForceSelectiveSuspend").unwrap_or(None);
        if cur.is_some() {
            backup.record(&radio_scope(inst), "ForceSelectiveSuspend", &cur.map_or(Orig::Absent, Orig::Dword));
            reg::delete_value(&key, "ForceSelectiveSuspend")
                .map_err(|e| format!("ForceSelectiveSuspend: {e}"))?;
            report.push(format!("[radio {inst}] ForceSelectiveSuspend removido"));
        }

        // USB Enhanced Power Management desligado (camada do stack USB)
        let cur = reg::read_dword(&key, "EnhancedPowerManagementEnabled").unwrap_or(None);
        backup.record(&radio_scope(inst), "EnhancedPowerManagementEnabled", &cur.map_or(Orig::Absent, Orig::Dword));
        reg::write_dword(&key, "EnhancedPowerManagementEnabled", 0)
            .map_err(|e| format!("EnhancedPowerManagementEnabled: {e}"))?;
        report.push(format!("[radio {inst}] EnhancedPowerManagementEnabled=0"));
    }

    // ---- 2) "Store Link Key": NAO e tocada ---------------------------------
    //
    // RE (bthport.SYS) mostrou que esse valor e um MARCADOR de status, gerido
    // pela propria pilha, e nao uma opcao de politica:
    //   FUN_1400a85c0  tratador do evento HCI "Write Link Key Complete":
    //                   quando o RADIO confirma que gravou a chave de link,
    //                   chama FUN_14005bd38(dev, 0x1000000, 1) -> liga o bit
    //                   0x1000000 dos flags do dispositivo (+0x368);
    //   FUN_1401c5a04  persiste "Store Link Key"=1 no registro do dispositivo
    //                   (BTHPORT\\Parameters\\Devices\\<MAC>) nesse caso;
    //   FUN_1401c522c  le o valor de volta no boot para restaurar o bit.
    // Ou seja: 1 = "o radio guardou a chave de link". Como este dongle so
    // guarda a chave na RAM (evento BTHUSB id 18: "o Windows nao pode
    // armazenar chaves de link no adaptador local"), o valor e reescrito
    // pela pilha a cada reinicio do radio e nao ha nada a configurar aqui.
    // Escrever 1 a mao seria mentir para a pilha, e apagar o valor seria
    // inutil (a pilha recria em segundos). Portanto o BTGuard nao toca nele.

    backup.save()?;
    log("INFO", &format!("endurecimento aplicado: {} acoes", report.len()));
    Ok(report)
}

/// Reverte usando o backup.
pub fn revert() -> Result<Vec<String>, String> {
    let lines = Backup::load()?;
    let mut report = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 3 {
            continue;
        }
        let (scope_kind, scope_id) = split_scope(&parts[0..2].join("|"));
        let name = parts[2];
        let (kind, val) = match parts.len() {
            3 => ("absent", ""),
            5 => (parts[3], parts[4]),
            _ => continue,
        };
        let path = match scope_kind.as_str() {
            "RADIO" => pnp::radio_device_params(&scope_id),
            "DEV" => format!(r"{}\{}", pnp::BTHPORT_DEVICES_KEY, scope_id),
            _ => continue,
        };
        let key = match reg::open_write(&path) {
            Ok(k) => k,
            Err(e) => {
                report.push(format!("[{scope_id}] NAO PUDOU ABRIR: {e}"));
                continue;
            }
        };
        match kind {
            "absent" => {
                let _ = reg::delete_value(&key, name);
                report.push(format!("[{scope_id}] {name} removido"));
            }
            "dword" => {
                let v: u32 = val.parse().unwrap_or(0);
                let _ = reg::write_dword(&key, name, v);
                report.push(format!("[{scope_id}] {name}={v}"));
            }
            "bytes" => {
                let b: Vec<u8> = (0..val.len() / 2)
                    .map(|i| u8::from_str_radix(&val[i * 2..i * 2 + 2], 16).unwrap_or(0))
                    .collect();
                let _ = reg::write_bytes(&key, name, &b, 3);
                report.push(format!("[{scope_id}] {name} restaurado"));
            }
            _ => {}
        }
    }
    log("INFO", &format!("endurecimento revertido: {} acoes", report.len()));
    Ok(report)
}

/// Estado atual do endurecimento (para o relatorio).
pub fn state() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for inst in pnp::radio_instances() {
        if let Ok(k) = reg::open_read(&pnp::radio_device_params(&inst)) {
            out.push((
                format!("radio {inst}"),
                format!(
                    "SelectiveSuspendSupported={:?} ForceSelectiveSuspend={:?} EnhancedPowerManagementEnabled={:?} DeviceSelectiveSuspended={:?}",
                    reg::read_dword(&k, "SelectiveSuspendSupported").unwrap_or(None),
                    reg::read_dword(&k, "ForceSelectiveSuspend").unwrap_or(None),
                    reg::read_dword(&k, "EnhancedPowerManagementEnabled").unwrap_or(None),
                    reg::read_dword(&k, "DeviceSelectiveSuspended").unwrap_or(None),
                ),
            ));
        }
    }
    for (addr, _n) in pnp::paired_devices() {
        if let Ok(k) = reg::open_read(&format!(r"{}\{}", pnp::BTHPORT_DEVICES_KEY, addr)) {
            out.push((
                format!("dev {addr}"),
                format!(
                    "Store Link Key={:?}",
                    reg::read_dword(&k, "Store Link Key").unwrap_or(None)
                ),
            ));
        }
    }
    out
}
