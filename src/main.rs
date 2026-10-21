// BTGuard - supervisor/reparador do radio Bluetooth USB (CSR Nanosira).
//
// Subcomandos:
//   status      diagnostico completo do radio, doBT e do endurecimento
//   harden      aplica as chaves de registro descobertas por RE no driver BthUsb
//   unharden    reverte o endurecimento (usa o backup)
//   recover     forca a reinicializacao do radio agora
//   watch       loop de vigilancia (usado pelo agendador de tarefas)
//   install     endurece + cria a tarefa agendada de vigilancia (auto-elevacao)
//   uninstall   remove a tarefa agendada + reverte o endurecimento

mod guard;
mod harden;
mod pnp;
mod reg;
mod util;

use util::log;

const TASK_NAME: &str = "BTGuard";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    let elevated = args.iter().any(|a| a == "--elevated");

    let code = match cmd {
        "status" => cmd_status(),
        "diag" => cmd_diag(),
        "harden" => {
            if elevated {
                cmd_harden()
            } else {
                relaunch_elevated("harden --elevated")
            }
        }
        "unharden" => {
            if elevated {
                cmd_unharden()
            } else {
                relaunch_elevated("unharden --elevated")
            }
        }
        "recover" => cmd_recover(),
        "watch" => cmd_watch(),
        "install" => {
            if elevated {
                cmd_install()
            } else {
                relaunch_elevated("install --elevated")
            }
        }
        "uninstall" => {
            if elevated {
                cmd_uninstall()
            } else {
                relaunch_elevated("uninstall --elevated")
            }
        }
        _ => {
            usage();
            0
        }
    };
    std::process::exit(code);
}

fn usage() {
    println!(
        "BTGuard 1.0 - supervisor do radio Bluetooth USB (CSR Nanosira)\n\n\
         Uso:\n\
         \x20 btguard status      diagnostico do radio, do BT e do endurecimento\n\
         \x20 btguard harden      aplica o endurecimento (requer administrador)\n\
         \x20 btguard unharden    reverte o endurecimento\n\
         \x20 btguard recover     reinicia o radio agora\n\
         \x20 btguard watch       vigilancia continua\n\
         \x20 btguard install     instala tudo (auto-elevacao)\n\
         \x20 btguard uninstall   remove tudo"
    );
}

fn relaunch_elevated(args: &str) -> i32 {
    let exe = current_exe();
    // Usa o proprio PowerShell para pedir a elevacao (UAC): mais simples e
    // equivalente ao ShellExecuteW "runas", sem depender de API extra.
    let ps = format!(
        "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs",
        exe.replace('\'', "''"),
        args.replace('\'', "''")
    );
    match std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .spawn()
    {
        Ok(_) => {
            println!("Solicitada elevacao (confirme o UAC).");
            0
        }
        Err(e) => {
            eprintln!("Falha ao solicitar elevacao: {e}");
            eprintln!("Execute manualmente em um terminal como administrador.");
            1
        }
    }
}

fn current_exe() -> String {
    unsafe {
        use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
        let mut buf = vec![0u16; 1024];
        let n = GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32);
        util::from_wide(&buf[..n as usize])
    }
}

fn cmd_status() -> i32 {
    println!("=== BTGuard - diagnostico ===");
    println!("hora: {}", util::ts_local(util::now_epoch()));

    println!("\n-- radio USB --");
    let instances = pnp::radio_instances();
    if instances.is_empty() {
        println!("  NENHUM radio USB\\VID_0A12&PID_0001 encontrado");
    }
    for inst in &instances {
        let full = format!("{}\\{}", pnp::RADIO_HWID, inst);
        let di = pnp::locate(&full);
        let (st, prob) = match di.and_then(pnp::status) {
            Some(v) => v,
            None => (0, 0),
        };
        println!(
            "  instancia {} | presente={} iniciado={} problema={}",
            inst,
            di.is_some(),
            st & 8 != 0,
            prob
        );
        if let Ok(k) = reg::open_read(&pnp::radio_device_params(inst)) {
            println!(
                "     SelectiveSuspendSupported={:?} ForceSelectiveSuspend={:?} EnhancedPowerManagementEnabled={:?} DeviceSelectiveSuspended={:?}",
                reg::read_dword(&k, "SelectiveSuspendSupported").unwrap_or(None),
                reg::read_dword(&k, "ForceSelectiveSuspend").unwrap_or(None),
                reg::read_dword(&k, "EnhancedPowerManagementEnabled").unwrap_or(None),
                reg::read_dword(&k, "DeviceSelectiveSuspended").unwrap_or(None)
            );
        }
    }

    println!("\n-- dispositivos Bluetooth pareados --");
    for (addr, name) in pnp::paired_devices() {
        let present = pnp::bt_device_present(&addr);
        let sk = reg::open_read(&format!(r"{}\{}", pnp::BTHPORT_DEVICES_KEY, addr))
            .ok()
            .and_then(|k| reg::read_dword(&k, "Store Link Key").ok().flatten());
        println!(
            "  {} {:<18} presente={} StoreLinkKey={:?}",
            addr, name, present, sk
        );
    }

    println!("\n-- endurecimento --");
    for (k, v) in harden::state() {
        println!("  {k}: {v}");
    }

    println!("\n-- ultimos eventos BTHUSB --");
    let evs = guard::recent_bth_events(12);
    if evs.is_empty() {
        println!("  (nenhum evento disponivel)");
    }
    for e in &evs {
        let what = match e.id {
            3 => "timeout de comando HCI",
            16 => "falha de autenticacao mutua",
            18 => "radio nao consegue guardar chaves de link",
            31 => "filtro de anuncio LE ausente",
            34 => "sem estado de controlador low-power",
            _ => "",
        };
        println!(
            "  id={:<3} {} {}",
            e.id,
            util::ts_local(e.epoch),
            what
        );
    }
    0
}

fn cmd_diag() -> i32 {
    println!("== dispositivos Bluetooth (API bluetoothapis) ==");
    for d in pnp::bt_devices() {
        println!(
            "  {} {:<24} cod=0x{:08x} conectado={} lembrado={} autenticado={}",
            d.addr, d.name, d.cod, d.connected, d.remembered, d.authenticated
        );
    }
    println!("== evento BTHUSB ==");
    print!("{}", guard::diag_evt());
    println!("== assinatura de eventos ==");
    print!("{}", guard::diag_sub());
    0
}

fn cmd_harden() -> i32 {
    match harden::apply() {
        Ok(rep) => {
            println!("Endurecimento aplicado:");
            for r in rep {
                println!("  {r}");
            }
            0
        }
        Err(e) => {
            eprintln!("Falha ao aplicar o endurecimento: {e}");
            1
        }
    }
}

fn cmd_unharden() -> i32 {
    match harden::revert() {
        Ok(rep) => {
            println!("Endurecimento revertido:");
            for r in rep {
                println!("  {r}");
            }
            0
        }
        Err(e) => {
            eprintln!("Falha ao reverter o endurecimento: {e}");
            1
        }
    }
}

fn cmd_recover() -> i32 {
    match guard::repair("comando manual") {
        Ok(true) => {
            println!("Radio reiniciado com sucesso.");
            0
        }
        Ok(false) => {
            println!("Radio reiniciado, mas nao voltou a funcionar.");
            1
        }
        Err(e) => {
            eprintln!("Falha: {e}");
            1
        }
    }
}

fn cmd_watch() -> i32 {
    guard::watch();
    0
}

fn cmd_install() -> i32 {
    let exe = current_exe();
    println!("Instalando BTGuard...");

    // ---- 1) Copia o executavel para o diretorio de instalacao -------------
    // A tarefa agendada aponta para essa copia: o projeto pode ser movido ou
    // apagado sem quebrar a vigilancia.
    let dir = match std::env::var("ProgramData") {
        Ok(p) if !p.is_empty() => std::path::PathBuf::from(p).join("BTGuard"),
        _ => {
            eprintln!("  [install] FALHOU: variavel de ambiente ProgramData ausente");
            return 1;
        }
    };
    let target = dir.join("btguard.exe");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("  [install] FALHOU ao criar {}: {e}", dir.display());
        return 1;
    }
    // A copia anterior pode estar em execucao (a tarefa de vigilancia): um
    // executavel em uso nao pode ser sobrescrito.
    let _ = std::process::Command::new("schtasks.exe")
        .args(["/End", "/TN", TASK_NAME])
        .output();
    let same = std::fs::canonicalize(&exe)
        .ok()
        .zip(std::fs::canonicalize(&target).ok())
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    if !same {
        // O processo antigo pode levar um instante para encerrar.
        let mut copied = false;
        let mut last_err = String::new();
        for attempt in 0..12 {
            match std::fs::copy(&exe, &target) {
                Ok(_) => {
                    copied = true;
                    break;
                }
                Err(e) => {
                    last_err = e.to_string();
                    let _ = std::process::Command::new("schtasks.exe")
                        .args(["/End", "/TN", TASK_NAME])
                        .output();
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = attempt;
                }
            }
        }
        if !copied {
            eprintln!(
                "  [install] FALHOU ao copiar para {}: {last_err}",
                target.display()
            );
            return 1;
        }
    }
    println!("  [install] executavel: {}", target.display());

    // ---- 2) Endurecimento --------------------------------------------------
    match harden::apply() {
        Ok(rep) => {
            for r in &rep {
                println!("  [harden] {r}");
            }
        }
        Err(e) => {
            eprintln!("  [harden] FALHOU: {e}");
            return 1;
        }
    }

    // O driver le "SelectiveSuspendSupported" no PnpStartDevice, ou seja, a
    // mudanca so vale depois que o radio reinicia. Sem isso o dongle continua
    // dormindo (DeviceSelectiveSuspended=1) mesmo com o registro corrigido.
    match guard::repair("instalacao (aplicar o endurecimento no radio)") {
        Ok(true) => println!("  [install] radio reiniciado: o endurecimento ja esta em vigor"),
        Ok(false) => println!("  [install] radio reiniciado, mas nao voltou a funcionar"),
        Err(e) => println!("  [install] AVISO: nao foi possivel reiniciar o radio: {e}"),
    }

    // ---- 3) Tarefa agendada -----------------------------------------------
    let task_exe = target.to_string_lossy().to_string();
    if let Err(e) = register_task(&task_exe) {
        eprintln!("  [tarefa] FALHOU: {e}");
        return 1;
    }
    println!("  [tarefa] '{TASK_NAME}' criada (boot +30s, SYSTEM, prioridade 7, reinicio automatico)");

    // ---- 4) Inicia a vigilancia imediatamente ------------------------------
    let _ = std::process::Command::new("schtasks.exe")
        .args(["/Run", "/TN", TASK_NAME])
        .output();

    log("INFO", "BTGuard instalado");
    println!("\nInstalado. Log em {}", util::log_dir().unwrap_or_default().display());
    println!("Para desinstalar: btguard uninstall");
    0
}

/// Registra a tarefa agendada que roda o vigia como SYSTEM no boot.
///
/// Usa o modulo ScheduledTasks do PowerShell: o validador XML do `schtasks`
/// rejeita `LogonType=ServiceAccount` (bug conhecido do Windows), o que impede
/// criar a tarefa de "Executar como SYSTEM" via `/XML`.
/// Se o modulo falhar, cai para `schtasks /Create` (sem atraso e sem
/// reinicio automatico, porem funcional).
fn register_task(exe: &str) -> Result<(), String> {
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$action   = New-ScheduledTaskAction -Execute '{exe}' -Argument 'watch'
$trigger  = New-ScheduledTaskTrigger -AtStartup
$trigger.Delay = 'PT30S'
$principal= New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
"#,
        exe = exe
    );
    let ps = std::env::temp_dir().join("btguard-register-task.ps1");
    std::fs::write(&ps, script).map_err(|e| format!("ao gravar script: {e}"))?;

    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &ps.to_string_lossy(),
        ])
        .output();
    let _ = std::fs::remove_file(&ps);

    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            // Fallback: CLI do schtasks (funciona, porem sem Delay/RestartOnFailure).
            let cli = std::process::Command::new("schtasks.exe")
                .args([
                    "/Create",
                    "/TN",
                    TASK_NAME,
                    "/TR",
                    &format!("{} watch", exe),
                    "/SC",
                    "ONSTART",
                    "/RU",
                    "SYSTEM",
                    "/RL",
                    "HIGHEST",
                    "/F",
                ])
                .output();
            match cli {
                Ok(c) if c.status.success() => Ok(()),
                Ok(c) => Err(format!(
                    "modulo ScheduledTasks: {err} | schtasks: {}",
                    String::from_utf8_lossy(&c.stderr).trim()
                )),
                Err(e) => Err(format!("modulo ScheduledTasks: {err} | schtasks: {e}")),
            }
        }
        Err(e) => Err(format!("powershell: {e}")),
    }
}

fn cmd_uninstall() -> i32 {
    let _ = std::process::Command::new("schtasks.exe")
        .args(["/End", "/TN", TASK_NAME])
        .output();
    let out = std::process::Command::new("schtasks.exe")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output();
    match out {
        Ok(o) if o.status.success() => println!("  [tarefa] removida"),
        _ => println!("  [tarefa] nao encontrada (ok)"),
    }
    match harden::revert() {
        Ok(rep) => {
            for r in rep {
                println!("  [unharden] {r}");
            }
        }
        Err(e) => eprintln!("  [unharden] {e}"),
    }

    // Remove o executavel instalado (o log e o backup sao preservados).
    if let Ok(pd) = std::env::var("ProgramData") {
        let exe = std::path::PathBuf::from(pd).join("BTGuard").join("btguard.exe");
        if exe.exists() {
            match std::fs::remove_file(&exe) {
                Ok(()) => println!("  [install] executavel removido"),
                Err(e) => eprintln!("  [install] nao foi possivel remover o exe: {e}"),
            }
        }
    }
    0
}
