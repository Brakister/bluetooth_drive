// Loop de vigilancia: observa o log de eventos do BTHUSB e repara o radio
// quando ele reinicia (o dongle CSR Nanosira reseta sozinho -> perde a chave de
// link -> o fone "desconecta" e o usuario era obrigado a reiniciar o PC).
//
// Sinais usados (todos observados empiricamente no log System deste PC):
//   BTHUSB id 31 / 34 / 18  -> o radio acabou de (re)inicializar
//   BTHUSB id 3              -> "Um comando enviado para o adaptador expirou"
//   BTHUSB id 16             -> falha de autenticacao mutua com o dispositivo

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::EventLog::*;

use crate::pnp;
use crate::util::log;

/// Diagnostico cru do Event Log.
pub fn diag_evt() -> String {
    unsafe {
        use windows_sys::Win32::Foundation::GetLastError;
        let mut s = String::new();
        for path_s in ["System", "{System}"] {
            for (label, q) in [
                ("todos", "*"),
                ("BTHUSB", "*[System[Provider[@Name='BTHUSB']]]"),
            ] {
                let path = crate::util::wide(path_s);
                let query = crate::util::wide(q);
                let h = EvtQuery(
                    0,
                    path.as_ptr(),
                    query.as_ptr(),
                    EvtQueryChannelPath | EvtQueryReverseDirection | EvtQueryTolerateQueryErrors,
                );
                let err = GetLastError();
                let mut out = format!("canal='{path_s}' {label}: h={h:?} lasterr={err}");
                if h != 0 {
                    let mut events: [isize; 1] = [0];
                    let mut returned: u32 = 0;
                    let ok = EvtNext(h, 1, events.as_mut_ptr(), 2000, 0, &mut returned);
                    out.push_str(&format!(
                        " next={ok} returned={returned} err={}",
                        GetLastError()
                    ));
                    if returned > 0 {
                        let mut used: u32 = 0;
                        let mut props: u32 = 0;
                        EvtRender(
                            0,
                            events[0],
                            EvtRenderEventXml,
                            0,
                            std::ptr::null_mut(),
                            &mut used,
                            &mut props,
                        );
                        out.push_str(&format!(" render_used={used} err={}", GetLastError()));
                        if used > 0 {
                            let mut buf = vec![0u16; (used as usize) / 2 + 2];
                            let ok2 = EvtRender(
                                0,
                                events[0],
                                EvtRenderEventXml,
                                (buf.len() * 2) as u32,
                                buf.as_mut_ptr() as *mut std::ffi::c_void,
                                &mut used,
                                &mut props,
                            );
                            out.push_str(&format!(" render2={ok2}"));
                            if ok2 != 0 {
                                let xml = String::from_utf16_lossy(&buf);
                                let head: String = xml.chars().take(400).collect();
                                out.push_str(&format!("\n   XML: {head}\n"));
                            }
                        }
                        EvtClose(events[0]);
                    }
                    EvtClose(h);
                }
                s.push_str(&out);
                s.push('\n');
            }
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct BthEvent {
    pub id: u32,
    pub epoch: u64,
}

/// Ultimo evento BTHUSB do log System (mais recente primeiro).
pub fn latest_bth_event() -> Option<BthEvent> {
    recent_bth_events(1).into_iter().next()
}

/// Le ate `limit` eventos do provider BTHUSB no log System (mais recentes primeiro).
pub fn recent_bth_events(limit: usize) -> Vec<BthEvent> {
    unsafe {
        let path = crate::util::wide("System");
        let query = crate::util::wide("*[System[Provider[@Name='BTHUSB']]]");
        let h = EvtQuery(
            0,
            path.as_ptr(),
            query.as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection | EvtQueryTolerateQueryErrors,
        );
        if h == 0 {
            log("WARN", "EvtQuery falhou (log System inacessivel?)");
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut events: Vec<isize> = vec![0isize; limit.max(1)];
        let mut returned: u32 = 0;
        let ok = EvtNext(
            h,
            limit as u32,
            events.as_mut_ptr(),
            3000,
            0,
            &mut returned,
        );
        if ok == 0 || returned == 0 {
            EvtClose(h);
            return Vec::new();
        }
        events.truncate(returned as usize);
        for ev in events {
            let xml = render_xml(ev);
            EvtClose(ev);
            if xml.is_empty() {
                continue;
            }
            let id = extract_event_id(&xml);
            let epoch = extract_system_time(&xml)
                .and_then(|s| parse_iso8601(s.trim_end_matches('Z')))
                .unwrap_or(0);
            out.push(BthEvent { id, epoch });
        }
        EvtClose(h);
        out
    }
}

/// Renderiza um evento como XML (buffer dimensionado em duas passadas).
fn render_xml(ev: isize) -> String {
    unsafe {
        let mut used: u32 = 0;
        let mut props: u32 = 0;
        EvtRender(
            0,
            ev,
            EvtRenderEventXml,
            0,
            std::ptr::null_mut(),
            &mut used,
            &mut props,
        );
        if used == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (used as usize) / 2 + 2];
        let ok = EvtRender(
            0,
            ev,
            EvtRenderEventXml,
            (buf.len() * 2) as u32,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            &mut used,
            &mut props,
        );
        if ok == 0 {
            return String::new();
        }
        let xml = String::from_utf16_lossy(&buf);
        // Corta no fim do elemento <Event ...> para reduzir memoria.
        match xml.find("</Event>") {
            Some(i) => xml[..i + 8].to_string(),
            None => xml,
        }
    }
}

/// Extrai o EventID. O XML usa aspas simples: <EventID Qualifiers='49157'>16</EventID>
fn extract_event_id(xml: &str) -> u32 {
    let start = match xml.find("<EventID") {
        Some(i) => i,
        None => return 0,
    };
    let gt = match xml[start..].find('>') {
        Some(i) => start + i + 1,
        None => return 0,
    };
    let end = match xml[gt..].find("</EventID>") {
        Some(i) => gt + i,
        None => return 0,
    };
    xml[gt..end].trim().parse::<u32>().unwrap_or(0)
}

/// Extrai o SystemTime (atributo TimeCreated).
fn extract_system_time(xml: &str) -> Option<String> {
    let i = xml.find("SystemTime=")? + "SystemTime=".len();
    let quote = xml[i..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let j = xml[i + 1..].find(quote)? + i + 1;
    Some(xml[i + 1..j].to_string())
}

/// Parse de "YYYY-MM-DDTHH:MM:SS[.frac]" -> epoch UTC.
fn parse_iso8601(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, n: usize| -> Option<i64> { s.get(a..a + n)?.parse::<i64>().ok() };
    let y = num(0, 4)?;
    let mo = num(5, 2)?;
    let d = num(8, 2)?;
    let h = num(11, 2)?;
    let mi = num(14, 2)?;
    let se = num(17, 2)?;
    let days = days_from_civil(y, mo, d);
    Some((days * 86_400 + h * 3600 + mi * 60 + se) as u64)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub struct GuardStats {
    pub radio_restarts: u64,
    pub hci_timeouts: u64,
    pub auth_failures: u64,
    pub recoveries: u64,
    pub last_event: Option<BthEvent>,
}

/// Executa uma acao de reparo: reinicia o devnode do radio (re-enumeracao USB).
pub fn repair(reason: &str) -> Result<bool, String> {
    log("WARN", &format!("reparo acionado: {reason}"));
    let (full, devinst) = match pnp::radio_any() {
        Some(x) => x,
        None => {
            log("ERROR", "radio nao encontrado no sistema");
            return Err("radio nao encontrado".into());
        }
    };
    log("INFO", &format!("reiniciando devnode do radio {full}"));
    pnp::restart_radio(devinst).map_err(|e| e.to_string())?;
    let ok = pnp::wait_started(devinst, 25);
    if ok {
        log("INFO", "radio voltou a funcionar");
    } else {
        log("ERROR", "radio NAO voltou a funcionar apos o reinicio");
    }
    Ok(ok)
}

/// Dispositivos pareados que deveriam estar conectados (Bluetooth classico).
fn expected_devices() -> Vec<String> {
    pnp::paired_devices()
        .into_iter()
        .map(|(addr, _)| addr)
        .collect()
}

/// Processa um evento BTHUSB. Retorna true se o evento foi "novo".
fn handle_event(ev: &BthEvent, st: &mut GuardStats, state: &mut WatchState) -> bool {
    if ev.epoch == 0 || ev.epoch <= state.last_epoch {
        state.last_epoch = state.last_epoch.max(ev.epoch);
        return false;
    }
    state.last_epoch = ev.epoch;
    st.last_event = Some(ev.clone());
    match ev.id {
        31 | 34 | 18 => {
            st.radio_restarts += 1;
            log(
                "WARN",
                &format!("radio reinicializou (evento BTHUSB id {})", ev.id),
            );
            // Da tempo para o radio subir e o dispositivo reconectar.
            std::thread::sleep(Duration::from_secs(4));
            match pnp::radio_healthy() {
                None => {
                    let _ = repair("radio nao subiu apos reinicializacao");
                    st.recoveries += 1;
                }
                Some(_) => {
                    std::thread::sleep(Duration::from_secs(8));
                    let expected = expected_devices();
                    let missing: Vec<String> = expected
                        .iter()
                        .filter(|a| !pnp::bt_device_present(a))
                        .cloned()
                        .collect();
                    if !missing.is_empty() && state.reconnect_grace.is_none() {
                        state.reconnect_grace = Some(Instant::now());
                        log("INFO", &format!("aguardando reconexao de {:?}", missing));
                    }
                }
            }
        }
        3 => {
            st.hci_timeouts += 1;
            log("ERROR", "timeout de comando HCI (evento BTHUSB id 3)");
            let _ = repair("timeout HCI");
            st.recoveries += 1;
        }
        16 => {
            st.auth_failures += 1;
            log("WARN", "falha de autenticacao mutua (evento BTHUSB id 16)");
        }
        _ => {}
    }
    true
}

pub struct WatchState {
    pub last_epoch: u64,
    pub reconnect_grace: Option<Instant>,
}

/// Contexto compartilhado entre o callback do EvtSubscribe e o laco principal.
struct SubCtx {
    queue: Mutex<VecDeque<BthEvent>>,
    /// Evento WIN32 sinalizado pelo callback para acordar o laco principal.
    signal: *mut std::ffi::c_void,
}

/// Callback do EvtSubscribe (executado numa thread do pool de eventos).
/// Apenas enfileira o evento: nada lento acontece aqui.
unsafe extern "system" fn on_event(
    action: i32,
    usercontext: *const std::ffi::c_void,
    event: isize,
) -> u32 {
    use windows_sys::Win32::System::Threading::SetEvent;
    if action != 1 {
        return 1; // so EvtSubscribeActionDeliver interessa
    }
    if usercontext.is_null() {
        return 1;
    }
    let ctx = &*(usercontext as *const SubCtx);
    let xml = render_xml(event);
    if !xml.is_empty() {
        let id = extract_event_id(&xml);
        let epoch = extract_system_time(&xml)
            .and_then(|s| parse_iso8601(s.trim_end_matches('Z')))
            .unwrap_or(0);
        if let Ok(mut q) = ctx.queue.lock() {
            if q.len() > 64 {
                q.pop_front();
            }
            q.push_back(BthEvent { id, epoch });
        }
    }
    // Acorda o laco principal imediatamente.
    let sig = ctx.signal;
    if !sig.is_null() {
        SetEvent(sig);
    }
    1
}

/// Diagnostico: testa as variantes de EvtSubscribe.
pub fn diag_sub() -> String {
    unsafe {
        use windows_sys::Win32::Foundation::GetLastError;
        let mut s = String::new();

        // Bookmark
        let path = crate::util::wide("System");
        let q_bth = crate::util::wide("*[System[Provider[@Name='BTHUSB']]]");
        let q_bth2 = crate::util::wide("*[Provider[@Name='BTHUSB']]");
        let h = EvtQuery(
            0,
            path.as_ptr(),
            q_bth.as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection | EvtQueryTolerateQueryErrors,
        );
        let mut events: [isize; 1] = [0];
        let mut returned: u32 = 0;
        let mut bm: isize = 0;
        if h != 0 && EvtNext(h, 1, events.as_mut_ptr(), 3000, 0, &mut returned) != 0 && returned > 0 {
            bm = EvtCreateBookmark(std::ptr::null());
            let ok = EvtUpdateBookmark(bm, events[0]);
            s.push_str(&format!(
                "bookmark={bm} update_ok={ok} err={}\n",
                GetLastError()
            ));
            EvtClose(events[0]);
        } else {
            s.push_str(&format!("EvtQuery falhou h={h} err={}\n", GetLastError()));
        }
        if h != 0 {
            EvtClose(h);
        }

        let ctx = std::ptr::null();
        for (label, query, use_bm, flags) in [
            ("A canal+bookmark+query(System)", q_bth.as_ptr(), true, EvtSubscribeStartAfterBookmark | EvtSubscribeTolerateQueryErrors),
            ("B canal+bookmark+query(sem System)", q_bth2.as_ptr(), true, EvtSubscribeStartAfterBookmark | EvtSubscribeTolerateQueryErrors),
            ("C canal+sem bookmark+query(System)", q_bth.as_ptr(), false, EvtSubscribeStartAfterBookmark | EvtSubscribeTolerateQueryErrors),
            ("D canal+sem bookmark+StartAtOldest", q_bth.as_ptr(), false, EvtSubscribeStartAtOldestRecord | EvtSubscribeTolerateQueryErrors),
            ("E sem canal+bookmark", std::ptr::null(), true, EvtSubscribeStartAfterBookmark | EvtSubscribeTolerateQueryErrors),
        ] {
            let path_arg = if label.starts_with('E') {
                std::ptr::null()
            } else {
                path.as_ptr()
            };
            let sub = EvtSubscribe(
                0,
                std::ptr::null_mut(),
                path_arg,
                query,
                if use_bm { bm } else { 0 },
                ctx,
                Some(on_event),
                flags,
            );
            s.push_str(&format!("{label}: sub={sub} err={}\n", GetLastError()));
            if sub != 0 {
                EvtClose(sub);
            }
        }
        if bm != 0 {
            EvtClose(bm);
        }
        s
    }
}

/// Cria um bookmark no ultimo evento BTHUSB ja registrado, para que a
/// assinatura entregue somente eventos novos.
fn last_bookmark() -> isize {
    unsafe {
        let path = crate::util::wide("System");
        let query = crate::util::wide("*[System[Provider[@Name='BTHUSB']]]");
        let h = EvtQuery(
            0,
            path.as_ptr(),
            query.as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection | EvtQueryTolerateQueryErrors,
        );
        if h == 0 {
            return 0;
        }
        let mut events: [isize; 1] = [0];
        let mut returned: u32 = 0;
        let ok = EvtNext(h, 1, events.as_mut_ptr(), 3000, 0, &mut returned);
        if ok == 0 || returned == 0 {
            EvtClose(h);
            return 0;
        }
        let bm = EvtCreateBookmark(std::ptr::null());
        if bm != 0 {
            EvtUpdateBookmark(bm, events[0]);
        }
        EvtClose(events[0]);
        EvtClose(h);
        bm
    }
}

/// Loop principal (usado pelo agendador de tarefas).
///
/// Modo PUSH: assina o log System via EvtSubscribe e dorme em
/// WaitForSingleObject. Em repouso o consumo de CPU e praticamente zero
/// (nenhuma consulta periodica ao log).
pub fn watch() {
    log("INFO", "BTGuard iniciado (modo vigilancia push)");
    let mut st = GuardStats {
        radio_restarts: 0,
        hci_timeouts: 0,
        auth_failures: 0,
        recoveries: 0,
        last_event: None,
    };
    let mut state = WatchState {
        last_epoch: 0,
        reconnect_grace: None,
    };
    // Prioridade baixa: nao pode atrapalhar o sistema.
    unsafe {
        use windows_sys::Win32::System::Threading::*;
        let p = GetCurrentProcess();
        SetPriorityClass(p, BELOW_NORMAL_PRIORITY_CLASS);
    }

    // Primeira passagem: popula o cursor sem disparar reparos por eventos antigos.
    if let Some(e) = latest_bth_event() {
        state.last_epoch = e.epoch;
    }

    unsafe {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

        let h_signal = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
        if h_signal.is_null() {
            log("ERROR", "CreateEventW falhou");
            return;
        }

        let ctx = Box::into_raw(Box::new(SubCtx {
            queue: Mutex::new(VecDeque::new()),
            signal: h_signal,
        })) as *const std::ffi::c_void;

        let bookmark = last_bookmark();
        let path = crate::util::wide("System");
        let query = crate::util::wide("*[System[Provider[@Name='BTHUSB']]]");
        let flags = EvtSubscribeStartAfterBookmark | EvtSubscribeTolerateQueryErrors;
        let mut sub = EvtSubscribe(
            0,
            h_signal,
            path.as_ptr(),
            query.as_ptr(),
            bookmark,
            ctx,
            Some(on_event),
            flags,
        );
        if sub == 0 {
            // O Windows recusa (erro 87) combinar um handle de evento de sinal
            // com um callback. Nao e falha: o callback chama SetEvent sozinho,
            // entao reassinamos sem o handle de sinal passado a EvtSubscribe.
            log(
                "INFO",
                &format!(
                    "EvtSubscribe com handle de sinal recusado ({}); usando variante so com callback",
                    windows_sys::Win32::Foundation::GetLastError()
                ),
            );
            sub = EvtSubscribe(
                0,
                std::ptr::null_mut(),
                path.as_ptr(),
                query.as_ptr(),
                bookmark,
                ctx,
                Some(on_event),
                flags,
            );
            if sub == 0 {
                log(
                    "ERROR",
                    &format!(
                        "EvtSubscribe (sem evento de sinal) falhou: {}",
                        windows_sys::Win32::Foundation::GetLastError()
                    ),
                );
            }
        }
        if bookmark != 0 {
            EvtClose(bookmark);
        }
        if sub == 0 {
            log("ERROR", "caindo para modo polling");
            // Fallback: polling simples a cada 3 s.
            loop {
                if let Some(e) = latest_bth_event() {
                    handle_event(&e, &mut st, &mut state);
                }
                check_grace(&mut st, &mut state);
                std::thread::sleep(Duration::from_secs(3));
            }
        }
        log("INFO", "assinatura do log System ativa (modo push)");

        loop {
            let r = WaitForSingleObject(h_signal, 5000);
            if r == WAIT_OBJECT_0 {
                let drained: Vec<BthEvent> = match (*(ctx as *const SubCtx)).queue.lock() {
                    Ok(mut q) => q.drain(..).collect(),
                    Err(_) => Vec::new(),
                };
                for ev in drained {
                    handle_event(&ev, &mut st, &mut state);
                }
            }
            check_grace(&mut st, &mut state);
        }
    }
}

/// Verifica dispositivos que nao voltaram dentro da janela de graca.
fn check_grace(st: &mut GuardStats, state: &mut WatchState) {
    if let Some(t0) = state.reconnect_grace {
        if t0.elapsed() > Duration::from_secs(45) {
            state.reconnect_grace = None;
            let expected = expected_devices();
            let missing: Vec<String> = expected
                .iter()
                .filter(|a| !pnp::bt_device_present(a))
                .cloned()
                .collect();
            if !missing.is_empty() {
                log("WARN", &format!("reconexao falhou para {:?}", missing));
                let _ = repair("dispositivo nao reconectou apos reset do radio");
                st.recoveries += 1;
            }
        }
    }
}

