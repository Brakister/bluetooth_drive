# Relatório técnico — por que o OBS derrubava o Bluetooth (e como foi resolvido)

**Máquina:** Windows 11 build 26100 (`10.0.26100.8655`), x64, fuso
`E. South America Standard Time` (UTC−3, sem DST).
**Adaptador:** `USB\VID_0A12&PID_0001\6&33C81247&0&6` — dongle Bluetooth USB
**CSR Nanosira (clone)**, driver `BthUsb` (`bth.inf`), serviço `BTHUSB`.
**Perfil:** fone **H02D Pro Anc** (VID/PID `000105D6`/`000A`),
`ee:e7:a9:aa:a8:af`, COD `0x00240404`.

> Método: engenharia reversa dos drivers da Microsoft (`BTHUSB.SYS`,
> `bthport.SYS`) com Ghidra/REA, correlação com o Event Log do Windows e com o
> log do OBS. Todas as afirmações abaixo têm evidência citada.

---

## Sumário

1. [Síntese](#1-síntese)
2. [A linha do tempo da falha](#2-a-linha-do-tempo-da-falha)
3. [O gatilho: por que o OBS derrubava o Bluetooth](#3-o-gatilho-por-que-o-obs-derrubava-o-bluetooth)
4. [Por que o rádio reiniciava sozinho](#4-por-que-o-rádio-reiniciava-sozinho)
5. [A chave de link: onde ela morre](#5-a-chave-de-link-onde-ela-morre)
6. [Engenharia reversa](#6-engenharia-reversa)
7. [A correção](#7-a-correção)
8. [Validação](#8-validação)
9. [O que ainda é problema do hardware](#9-o-que-ainda-é-problema-do-hardware)
10. [Recomendação de hardware](#10-recomendação-de-hardware)
11. [Anexo: como reproduzir e coletar evidências](#11-anexo-como-reproduzir-e-coletar-evidências)

---

## 1. Síntese

Três fatos independentes, confirmados por evidência, formam a falha:

| # | Fato | Evidência |
|---|---|---|
| A | O dongle entra em *selective suspend* USB e, ao acordar, **o firmware reinicia o controlador Bluetooth** | `DeviceSelectiveSuspended = 1`; `BTHUSB!FUN_14001882c` lê `SelectiveSuspendSupported`; sequência de ids 18/34/31 = "rádio inicializou" |
| B | A chave de link do pareamento é **volátil** (só existe na RAM do host; o dongle não a guarda e o registro também não) | Evento `BTHUSB` id 18: *"O Windows não pode armazenar códigos de autenticação Bluetooth (chaves de link) no adaptador local"*; varredura binária de todo o `BTHPORT` + `Control\Bluetooth`: **zero** valores de chave de link |
| C | Cada reinício do rádio ⇒ **falha de autenticação mútua** ⇒ fone desconectado até reiniciar o PC | `BTHUSB` id 16: *"Falha na autenticação mútua ... (ee:e7:a9:aa:a8:af)"*, seguido de id 3 (*timeout* HCI) 60 s depois |

O OBS era o gatilho mais frequente (e o mais visível), não a causa.

---

## 2. A linha do tempo da falha

### 2.1 O incidente com o OBS (log `2026-10-21 15-44-17.txt`)

```
15:44:21.999  OBS inicializa o endpoint A2DP   "Fones de ouvido (H02D Pro Anc)" 48 kHz
15:44:22.627  OBS inicializa o endpoint HFP     "Headset (H02D Pro Anc)" 16 kHz  (fonte Mic/Aux)
15:44:30.433  A2DP invalidado + saída padrão trocada para a onboard Realtek
15:49:20      OBS fecha
15:49:49.047  BTHUSB id 18  (rádio reiniciou)      <-- 29 s depois de o OBS fechar
15:49:49.111  BTHUSB id 34
15:49:49.117  BTHUSB id 31
15:49:49.941  BTHUSB id 16  (falha de autenticação mútua com ee:e7:a9:aa:a8:af)
15:50:49      BTHUSB id 3   (timeout de comando HCI, +60 s)
15:50:50      BTHUSB id 16  (nova tentativa de autenticação)
```

O mesmo padrão (ids 18/34/31 → 16 → 3) aparece em 08:13:08 e 08:34:44 do
mesmo dia e em outros dias.

### 2.2 O rádio reiniciava sozinho o dia inteiro

Trecho do log `System` (provider `BTHUSB`), **com as datas**: cada par
`18 → 34 → 31` é uma **reinicialização do controlador**, e occurs dozens de
vezes por dia, sem nenhum outro evento no log:

```
21/10 15:49:49  18 → 34 → 31   +16 (0,9 s)   +3 (60 s)
21/10 16:39:55  18 → 34 → 31   +16 (0,3 s)   +3 (60 s)     <- reinício manual (teste)
21/10 16:44:44  18 → 34 → 31
21/10 16:45:32  18 → 34 → 31
21/10 17:03:30  18 → 34 → 31
21/10 17:05:14  18 → 34 → 31   +16            <- reinício manual (instalação)
```

Já em outros dias (14/10, 18/10, 19/10, 20/10) o mesmo: reinicializações
espalhadas ao longo do dia, cada uma seguida de id 16 (auth falhou) e id 3
(timeout HCI) quando o fone tentava reconectar.

**Interpretação:** o rádio não reinicia por causa do OBS — reinicia o dia
inteiro. O OBS apenas (a) acorda o rádio no pior momento e (b) segura o
perfil de áudio, então a queda fica visível e é atribuída ao OBS.

### 2.3 Depois do endurecimento

| Horário | Evento |
|---|---|
| 21/10 17:05:14 | reinício do rádio (instalação, para aplicar o endurecimento) → id 18/34/31 → id 16 |
| 21/10 17:11:42 | reinício do rádio (instalação final) → id 18/34/31 |
| 21/10 17:11 → | **nenhuma reinicialização espontânea** |

---

## 3. O gatilho: por que o OBS derrubava o Bluetooth

O OBS tinha a fonte **Mic/Aux** (`wasapi_input_capture`) com `device_id = "default"`.

Com um fone Bluetooth conectado, o Windows promove o endpoint **HFP**
(*Hands-Free AG Audio*, 16 kHz mono) do fone a microfone padrão do sistema.
Ao abrir esse endpoint, o host **troca o perfil do fone de A2DP (48 kHz) para
HFP (16 kHz)** — e essa troca acorda/reinicia o dongle.

Evidência direta no log do OBS: o endpoint A2DP é inicializado às 15:44:21.999
e o endpoint HFP às 15:44:22.627; **8,4 s depois** o dispositivo A2DP é
invalidado (15:44:30.433) e a saída padrão muda para a onboard Realtek.

Endpoints no sistema (enumerados via WASAPI):

| Endpoint | ID | Estado | Papel |
|---|---|---|---|
| `H02D Pro Anc` (A2DP) | `{40d002f1…}` | ativo | saída de música |
| `H02D Pro Anc Hands-Free` | `{d5cbd511…}` | state 8 | **microfone HFP — o gatilho** |
| `GENERAL WEBCAM` | `{0120b34b…}` | ativo | microfone da webcam — **o destino correto** |
| onboard AMD / Realtek | — | state 4 | mics do notebook |

**Correção:** `install/Fix-OBSAudio.ps1` reescreve **apenas** o valor
`AuxAudioDevice1.settings.device_id` de `"default"` para
`{0.0.1.00000000}.{0120b34b-9c15-4771-9275-ce425f8a74cd}` (o microfone da
webcam), deixando `DesktopAudioDevice1` em `"default"`. A edição é cirúrgica
(o arquivo não é resserializado): diff byte a byte = **exatamente uma linha**,
529 linhas antes e depois, +48 bytes, acentuação preservada (o script lê e
grava em UTF‑8 explícito — o PowerShell 5.1 lê UTF‑8 sem BOM como ANSI e
corromperia os acentos).

Backup: `%APPDATA%\obs-studio\basic\btguard-backup\`.

---

## 4. Por que o rádio reiniciava sozinho

Engenharia reversa de `C:\Windows\System32\drivers\BTHUSB.SYS`
(sha256 `ca135f9a74ff3fead43fc26feff5abba830868f2140c8a432b007c3793a70e6d`).

### `BTHUSB!FUN_14001882c` — as chaves de suspensão

```c
byte FUN_14001882c(longlong *param_1) {
  // abre a chave do dispositivo (Device Parameters)
  iVar4 = IoOpenDeviceRegistryKey(param_1[2], 1, 0x20000, local_d8);
  if (iVar4 < 0) goto default_path;

  iVar4 = read(local_d8[0], L"ForceSelectiveSuspend", local_e4);
  bVar5 = 0;
  if (iVar4 >= 0 && local_e4[0] == 1) bVar5 = 1;      // 1 => FORÇA o suspend
  ZwClose(local_d8[0]);
  if (bVar5 == 0) goto default_path;

  // ForceSelectiveSuspend == 1:
  *(byte *)(*(longlong *)(lVar2 + 0x68) + 7) |= 0x60;  // liga os bits de capability USB
  *(byte *)(lVar2 + 0x492) = 1;                        // "suspend forçado"
  return 1;

default_path:
  // consulta as informações do nó USB (desc do dispositivo)
  iVar4 = FUN_14001cf28(..., 500, lVar2 + 0x30);
  *(byte *)(lVar2 + 0x492) = local_e8[0] & 1;          // o device declara suporte a SS?
  *(byte *)(lVar2 + 0x490) = (*(byte *)(lVar3 + 7) >> 5) & 1;
  *(byte *)(lVar2 + 0x491) = (*(byte *)(lVar3 + 7) >> 6) & 1;
  if (((local_e8[0] & 1) == 0) || ((*(byte *)(lVar3 + 7) & 0x60) != 0x60))
      goto done;                                        // device não suporta SS
  // reabre a chave e lê "SelectiveSuspendSupported"
  iVar4 = read(local_d8[0], L"SelectiveSuspendSupported", local_e4);
  if (iVar4 >= 0 && local_e4[0] == 0) {
      *(byte *)(lVar2 + 0x493) = 1;                     // DESABILITA o suspend seletivo
      bVar5 = 0;
  }
done:
  *(byte *)(lVar2 + 0x494) = 1;
  return bVar5;
}
```

Tradução:

- `ForceSelectiveSuspend = 1` ⇒ o driver **força** o *selective suspend* USB.
  → **não** está presente nesta máquina (o que é bom).
- `SelectiveSuspendSupported = 0` ⇒ o driver **desabilita** o suspend seletivo
  daquele rádio. **Originalmente ausente** ⇒ o driver usava a declaração do
  descritor USB do dongle (que diz suportar suspensão) ⇒ o dongle dormia.
- Corroboração: `DeviceSelectiveSuspended = 1` no registro do rádio — o
  dispositivo **estava** suspenso.

O clone CSR Nanosira **não sobrevive a um ciclo de suspensão**: ao acordar, o
firmware reinicializa o controlador (é isso que produz a sequência
`18 → 34 → 31`).

> **Nota de engenharia:** o driver lê essas chaves no `PnpStartDevice`. Alterar
> o registro **não** basta — o rádio precisa ser reiniciado para que a decisão
> nova entre em vigor. O `btguard install` faz isso (reinicia só o dongle).

### `EnhancedPowerManagementEnabled`

Não aparece em `BTHUSB.SYS` (é uma chave do próprio *stack* USB/WDF). O
BTGuard a zera por precaução, junto com a chave principal.

---

## 5. A chave de link: onde ela morre

### 5.1 O evento 18

```
BTHUSB id 18 (Informativo):
"O Windows não pode armazenar códigos de autenticação Bluetooth (chaves de link)
 no adaptador local. Os teclados Bluetooth podem não funcionar no BIOS do
 sistema durante a inicialização."
```

Tradução: **o Windows não consegue guardar a chave de link *no adaptador*.**
Logo, a chave **não** vive no dongle. Cada reinício do controlador ⇒ o host
perde a chave ⇒ o fone tenta reconectar com a chave antiga ⇒ **id 16** (falha
de autenticação mútua) ⇒ id 3 (timeout HCI) 60 s depois.

### 5.2 Onde a chave de link realmente vive

Três fontes de evidência, convergentes:

| Onde? | Verificação | Resultado |
|---|---|---|
| **no dongle** | evento BTHUSB id 18 (o Windows não consegue gravar chaves no adaptador) | **não** — o rádio não tem onde guardar |
| **no registro** | varredura binária de `HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT` (todos os dispositivos) + `Control\Bluetooth` | **não** — nenhum blob de 16 bytes; só o marcador `Store Link Key = 1` (ver [5.4](#54-store-link-key--o-que-ela-realmente-é)) |
| **na RAM do host** | o par é revalidado a cada conexão; a chave só existe enquanto a pilha está viva | **sim** |

Ou seja: **a chave de link é volátil**. Perde-se quando o rádio reinicia **ou
quando o PC reinicia**. Este é o ponto central do problema: *"o Bluetooth só
voltava depois de reiniciar o PC"* não era mágica do reboot — era o
**re-pareamento** que o reboot forçava, e que hoje o BTGuard faz
automaticamente.

### 5.3 Varredura do registro (detalhe)

Não existe **nenhuma** chave de link guardada pelo host:

- `HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters\Devices\eee7a9aaa8af`
  → `COD`, `Name`, `FriendlyName`, `VID`, `PID`, `LMPFeatures`,
  `HostSupportedFeaturesMap`, `FingerprintString`, `LastSeen`, `LastConnected`,
  `LocalEvaldIoCap`, `ManufacturerId`, `LmpVersion`, `LmpSubversion`,
  `DibServiceVersion`, `Version`, `VIDType`, `FingerprintVersion`,
  `FingerprintTimestamp` — **nenhum valor de chave de link**.
- `…\BTHPORT\Parameters\ServicesFor001a7dda7113\*` → parâmetros SSP
  (`SSP Paired`, `SSP Supported`, `IoCapability`, `AuthenticationRequirements`,
  `RemoteAuthenticationRequirements`, `BasebandSupport`) — **também sem chave**.
- `…\BTHPORT\Parameters\CachedServices\*`, `…\0\*` (instância do próprio rádio,
  `AssocBdAddr` = o fone) — sem chave de link.

### 5.4 `Store Link Key` — o que ela **realmente** é

Engenharia reversa de `bthport.SYS`
(sha256 `f0d8a8de34443467bfe54f724d2fbd591f3fbc260d13f43f2e87eb0ba7301673`).

Uma primeira hipótese era "`Store Link Key = 1` ⇒ o host guarda a chave". A RE
**refutou** essa hipótese:

```c
// bthport!FUN_1400a85c0  — tratador do evento HCI "Write Link Key Complete"
...
lVar7 = FUN_140060700(param_1, puVar1, 0, FUN_1400a85c0, 0);
if (lVar7 == 0) { ... 0xc0000141 ... }
KeEnterCriticalRegion();
ExAcquireFastMutexUnsafe(lVar7 + 0x380);
FUN_14005bd38(lVar7, 0x1000000, 1);      // <-- liga o bit "a chave está no rádio"
```

```c
// bthport!FUN_14005bd38  — setter genérico de flags do dispositivo (+0x368)
uVar1 = *(ulonglong *)(param_1 + 0x368);
if (param_3 == 0) { LOCK(); *(ulonglong *)(param_1+0x368) &= ~param_2; UNLOCK(); }
else               { LOCK(); *(ulonglong *)(param_1+0x368) |=  param_2; UNLOCK(); }
uVar2 = *(ulonglong *)(param_1 + 0x368);
if ((((uVar2 ^ uVar1) >> 0x18 & 1) != 0) &&          // o bit 24 mudou?
    ((*(uint *)(param_1 + 0x18) & 0x30018) != 0)) {
    if (KeGetCurrentIrql() < 2) FUN_1401c5a04(param_1);  // persiste no registro
}
```

```c
// bthport!FUN_1401c5a04  — grava no registro do dispositivo
if ((*(uint *)(param_1 + 0x18) & 0x8000) == 0) {
    iVar1 = FUN_1401c6a6c(param_1 + 0x20, local_res10, 1);   // abre a chave
    if (-1 < iVar1) {
        RtlInitUnicodeString(&local_18, L"Store Link Key");
        local_res8[0] = (uint)((*(ulonglong *)(param_1 + 0x368) & 0x1000000) != 0);
        if ((*(ulonglong *)(param_1 + 0x368) & 0x1000000) != 0)
            ZwSetValueKey(local_res10[0], &local_18, 0, 4, local_res8, 4);  // DWORD = 1
        ZwClose(local_res10[0]);
    }
}
```

```c
// bthport!FUN_1401c522c  — lê o valor de volta no boot
iVar2 = FUN_14020ead4(local_40, L"Store Link Key", local_38, local_48);
if (-1 < iVar2) {
    if (local_48[0] == 0) { LOCK(); *(ulonglong*)(param_1+0x368) &= ~0x1000000; UNLOCK(); }
    else                   { LOCK(); *(ulonglong*)(param_1+0x368) |=  0x1000000; UNLOCK(); }
}
```

**Conclusão:** `Store Link Key` é um **marcador de status** — *"o rádio
confirmou que gravou a chave de link"* — e **não** uma opção de política. O
Windows o grava quando o rádio responde com sucesso ao comando HCI *Write
Link Key* e o relê no boot para restaurar o bit de flags.

**Confirmação empírica:** o BTGuard removeu o valor às 17:05:12 e, **60 s
depois** do reinício do rádio, a pilha o reescreveu sozinho como `1`. Logo, é
um valor gerido pelo Windows — e o BTGuard **não toca nele** (escrever `1` à
mão seria mentir para a pilha; apagar seria inútil).

> Observação de RE: `bth.inf` (Microsoft) grava `Store Link Key COD Masks` no
> `Device Parameters` de **todo** rádio Bluetooth:
> ```
> HKR,,"Store Link Key COD Masks", 0x00000001, 00,00,1F,43, 00,00,05,40, 00,00,1F,83, 00,00,05,80
> ```
> É a lista (COD, máscara) de classes de dispositivo cujas chaves o rádio deve
> guardar.

---

## 6. Engenharia reversa

| Alvo | SHA-256 | O que foi feito |
|---|---|---|
| `C:\Windows\System32\drivers\BTHUSB.SYS` | `ca135f9a74ff…a70e6d` | `FUN_14001882c` (suspensão), strings `ForceSelectiveSuspend`, `SelectiveSuspendSupported`, `SelectiveSuspendDisabledViaRegistry` |
| `C:\Windows\System32\drivers\bthport.SYS` | `f0d8a8de3444…301673` | `FUN_1400a85c0` (Write Link Key Complete), `FUN_14005bd38` (setter de flags), `FUN_1401c5a04` (persistência), `FUN_1401c522c` (leitura no boot) |

Ferramenta: **Ghidra 12.1.4** via REA (análise efêmera, somente leitura).

Detalhes de operação que ficaram registrados (úteis para quem for repetir):

- `search_bytes` **trunca o padrão em 10 bytes** — padrões maiores nunca casam.
  Foi por isso que "nada encontrado" nas primeiras buscas.
- `0x368` em *little-endian* é `68 03 00 00` (não `68 12 00 00`).
- O *bit* que o RE procura é testado com `LOCK OR/AND qword [rbx+0x368], imm32`
  (prefixo `F0 48 81 /1 …`).
- Strings de interesse em `bthport.SYS`: `Store Link Key` @ `0x140180518`,
  `Store Link Key COD Masks` @ `0x140183f20`, `AuthenticateIfLinkKeyPresent`
  @ `0x140184970`, `Change Link Key Always` @ `0x140184940`,
  `AllowLegacyCombinationLinkKeys` @ `0x140184490`.

---

## 7. A correção

### 7.1 Endurecimento do driver (registro)

Em `HKLM\SYSTEM\CurrentControlSet\Enum\USB\VID_0A12&PID_0001\<inst>\Device Parameters`:

| Valor | Antes | Depois | Efeito (por RE) |
|---|---|---|---|
| `SelectiveSuspendSupported` | *ausente* | `0` | driver **desabilita** o suspend seletivo → o rádio **não dorme** → **não reinicia o controlador** |
| `EnhancedPowerManagementEnabled` | *ausente* | `0` | desliga o gerenciamento de energia estendido do USB |

Aplicado nas **4 instâncias** do rádio (`6&3353205&0&2`, `6&33c81247&0&11`,
`6&33c81247&0&14`, `6&33c81247&0&6` — só a última está presente).
Backup: `%ProgramData%\BTGuard\backup-hardening.txt`; reversão com
`btguard unharden`.

### 7.2 Remover o gatilho do OBS

`install/Fix-OBSAudio.ps1` → `AuxAudioDevice1.settings.device_id` passa a
apontar para o microfone da webcam (endpoint não-Bluetooth), em vez de
`"default"` (que o Windows resolve para o HFP do fone).

### 7.3 Vigia e recuperação automática

`btguard watch` (tarefa `BTGuard`, `SYSTEM`, boot +30 s, prioridade 7,
`RestartOnFailure`, `ExecutionTimeLimit = 0`):

- assinatura **push** do log `System` (`EvtSubscribe` + *callback* + `SetEvent`
  + `WaitForSingleObject`) — sem *polling* do log;
- **decide pela causa real, não pelo sintoma**:

  | Situação | Ação |
  |---|---|
  | id 18 (rádio reiniciou) e o fone volta em ~12 s | nada |
  | id 18 e o fone **não** volta, mas o rádio responde | **re-pareia** (a chave é volátil) |
  | id 18 e o fone não volta e o rádio está **travado** | reinicia só o devnode do dongle |
  | id 16 (falha de autenticação) e o fone não volta em 45 s | **re-pareia** |
  | id 3 (timeout HCI) **logo após** um id 16 | nada (é consequência do pareamento velho) |
  | id 3 isolado (rádio travado) | reinicia só o devnode do dongle |

- **nunca entra em laço**: intervalo mínimo de 3 min entre reparos, recuo
  exponencial até 30 min, teto de 6 tentativas seguidas, e uma janela em que os
  eventos gerados pelo *próprio* reparo são ignorados;
- log em `%ProgramData%\BTGuard\btguard.log` (com rotação).

### 7.4 Pareamento via API do Windows (`src/pair.rs`)

Re-parear pela interface do Windows (Settings) é lento e frágil. O BTGuard usa
a mesma API que o Settings usa — `bluetoothapis.dll` +
`bthprops.cpl` (`BluetoothFindFirstRadio`, `BluetoothFindFirstDevice` com
`fIssueInquiry`, `BluetoothAuthenticateDevice`, `BluetoothSetServiceState`),
sem automação de interface:

| Comando | O que faz |
|---|---|
| `btguard pair [nome]` | varre em modo pareamento, emparelha com SSP (Just Works) e religa os serviços de áudio |
| `btguard unpair [endereço]` | remove o pareamento |
| `btguard audio [endereço]` | religa só os serviços de áudio (A2DP/HFP) de um dispositivo já pareado |

**Armadilha real da API (encontrada e corrigida na implementação):**
`BluetoothSetServiceState` com `dwServiceFlags = 0` significa **DESLIGAR** o
serviço — *não* "consultar". A primeira versão do BTGuard usava isso como se
fosse consulta e **desligava silenciosamente o A2DP e o HFP**; o fone caía em
~3 s e parecia "desconectar sozinho". A correção:

- `flags = 0x01` (`BLUETOOTH_SERVICE_ENABLE`) → habilita;
- o estado real é lido do **registro** (`BTHPORT\Parameters\Devices\<addr>`),
  porque o Windows não expõe consulta de serviço remoto pela API pública;
- `ERROR_SERVICE_DOES_NOT_EXIST` (1060) = o fone não tem aquele perfil →
  informativo; `ERROR_SERVICE_ALREADY_ENABLED` (87) = já estava ligado →
  também informativo (não é erro).

**Re-pareamento não destrutivo:** `pair::re_pair()` exporta a chave de registro
do dispositivo (`reg.exe export`) antes de removê-la e **restaura o backup** se
o pareamento falhar (por exemplo, o fone está desligado e não aparece na
varredura). O vigia, assim, nunca destrói um pareamento que ainda pode
servir.

### 7.5 Detecção do "dispositivo esquecido"

Estado pós-reboot (chave de link perdida): o registro ainda registra o
dispositivo, mas a pilha Bluetooth já não o conhece. O vigia distingue:

- dispositivo **conhecido e desconectado** → o fone está desligado → *não faz
  nada*;
- dispositivo **desconhecido da pilha** (e presente no registro) → o host
  perdeu a chave → **re-pareia**.

Essa verificação roda na largada do vigia (60 s após o start) e a cada
janela de reconexão.

### 7.4 Nota sobre o registro da tarefa agendada

`schtasks /Create /XML` **rejeita** o XML desta tarefa neste Windows:
`ERRO: O XML da tarefa contém um valor formatado incorretamente ou fora do
intervalo. (5,91):LogonType:ServiceAccount` — bug conhecido do validador do
Windows (o enumerado de `LogonType` do XML não inclui `ServiceAccount`).
Resolução: o BTGuard registra a tarefa pelo módulo **`ScheduledTasks`** do
PowerShell (`New-ScheduledTaskTrigger -AtStartup` + `.Delay = 'PT30S'`,
`New-ScheduledTaskPrincipal -UserId SYSTEM -LogonType ServiceAccount
-RunLevel Highest`, `New-ScheduledTaskSettingsSet -RestartCount 999
-RestartInterval 1min -ExecutionTimeLimit 0 -StartWhenAvailable
-MultipleInstances IgnoreNew`), com *fallback* para `schtasks /Create` (CLI).

---

## 8. Validação

| Verificação | Resultado |
|---|---|
| `btguard install --elevated` | 8 ações de endurecimento + reinício do rádio + tarefa registrada e em execução |
| Tarefa `BTGuard` | `Status: Em execução`, roda como `SYSTEM` |
| Vigia | log: `assinatura do log System ativa (modo push)` |
| Reinício do rádio (teste manual) | *"radio voltou a funcionar"* em ~2 s; ids 18/34/31 + 16 emitidos, como previsto |
| Reinicializações espontâneas após o endurecimento | **nenhuma** (17:05 → 17:11 → horário posterior) |
| `Store Link Key` | reescrito pela pilha ⇒ confirmado como marcador gerido pelo Windows |
| Correção do OBS | aplicado; diff de 1 linha; backup criado |
| **Re-pareamento via API** (17:21) | `BTHUSB id 10` (chave removida) às 17:21:11 → `BTHUSB id 8` (*vinculado com sucesso*) às 17:21:24; serviços **Audio Sink** (A2DP) e **Handsfree** (HFP) ativos |
| **Re-conexão após reinício do rádio** | o fone volta **sozinho** em ~59 s (id 8) — sem reiniciar o PC |
| **OBS aberto com o fone pareado** (17:24:29) | log do OBS: `WASAPI: Device 'Microfone (GENERAL WEBCAM)' initialized (source: Mic/Aux)`; **zero** eventos BTHUSB até 17:29 ⇒ o gatilho foi eliminado |
| **Auto-re-pareamento** (simulação pós-reboot) | o vigia detecta o dispositivo sem chave, refaz o pareamento e religa o A2DP — sem reboot e sem ação do usuário |

### 8.1 Linha do tempo da sessão de reparo (21/10)

```
17:18:12  vigia instalado (tarefa agendada, SYSTEM)
17:19:13  id 3 (timeout HCI) -> reinicio do radio -> "radio voltou a funcionar"
17:19:15  id 34 (informativo) na sequencia do proprio reparo
17:21:11  BTHUSB id 10   -> chave de link removida (despareamento proposital)
17:21:24  BTHUSB id 8    -> vinculado com sucesso (re-pareamento via API do Windows)
17:22:56  btguard recover -> reinicio do radio (teste)
17:22:58  "radio voltou a funcionar"
17:24:29  OBS aberto -> WASAPI inicializa o microfone da WEBCAM (gatilho eliminado)
17:22:58 -> 17:29:xx   ZERO eventos BTHUSB (nenhum reinicio de radio ao abrir o OBS)
17:30:18  BTHUSB id 10   -> chave removida (simulacao do estado pos-reboot)
17:32:31  radio reiniciado -> id 18/34/31
17:33:30  BTHUSB id 8    -> o fone reconectou sozinho (59 s), sem reiniciar o PC
17:34:30  vigia novo build: janela de graca 45 s -> 90 s + recuperacao de largada
```

### 8.2 O que o teste do OBS provou

Antes: `Mic/Aux` do OBS apontava para `"default"` → o Windows resolvia para o
endpoint **HFP do fone** → o perfil do fone era trocado (A2DP → HFP) → o
rádio acordava/reiniciava → a chave de link morria → o fone caía.

Depois: `Mic/Aux` aponta para o microfone da webcam
(`{0120b34b-…}`, `GENERAL WEBCAM`). O log do OBS de 17:24:29 mostra o
endpoint sendo inicializado e **nenhum** evento BTHUSB foi emitido até 17:29 —
o OBS parou de ser o gatilho.

---

## 9. O que ainda é problema do hardware

Mesmo com o driver endurecido, o dongle **clone CSR Nanosira** continua com
duas deficiências de hardware/firmware:

1. **Sem armazenamento persistente de chaves de link.** O evento id 18 é
   emitido em **todo** reinício do rádio. Se o firmware reiniciar fora do
   controle do Windows (falha de alimentação, dongle defeituoso), o pareamento
   é perdido e o fone precisa ser pareado de novo — o BTGuard não tem como
   evitar isso.
2. **Reinicializações espontâneas.** O dongle reiniciou dezenas de vezes por
   dia mesmo sem nada acontecendo. O endurecimento elimina a causa conhecida
   (suspensão USB), mas um dongle com fonte ruim ou firmware instável ainda
   pode reiniciar sozinho.

Além disso, o clone não implementa corretamente os requisitos de LE
(erros id 34 e id 31 em todo reinício) — o que afeta recursos de
*Bluetooth Low Energy* (fones TWS, FIND MY, etc.), embora o A2DP/HFP
funcione.

---

## 10. Recomendação de hardware

| Opção | Notas |
|---|---|
| **Intel AX210 / AX211** | melhor suporte no Windows 11, armazenamento persistente de chaves de link, firmware estável |
| **Realtek RTL8761B** | custo-benefício, bom suporte a LE |
| **CSR Nanosira genuíno** | precisa ser **original** (CSR/CSr8510) — os clones é que falham |
| Evitar | clones genéricos sem marca (`VID_0A12&PID_0001` é o PID de vários clones baratos) |

Se o dongle for mantido, vale registrar no `Device Parameters` do rádio o
`ForceSelectiveSuspend = 1`? **Não** — o valor força o *suspend*, o oposto do
que queremos. O BTGuard apenas garante que ele **não** existe.

---

## 11. Anexo: como reproduzir e coletar evidências

```powershell
# 1. estado do rádio, do pareamento e do endurecimento
btguard status

# 2. diagnóstico completo (dispositivos via Bluetooth API, log de eventos)
btguard diag

# 3. forçar um reinício do rádio (equivalente ao que estraga o pareamento)
btguard recover

# 4. ver a sequência de eventos BTHUSB
Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='BTHUSB'} -MaxEvents 50 |
  Sort-Object TimeCreated | Format-Table TimeCreated, Id, Message -Wrap

# 5. log do OBS (o gatilho)
Get-Content "$env:APPDATA\obs-studio\logs\*.txt" | Select-String "A2DP|HFP|Hands-Free"

# 6. endpoints de áudio (qual endpoint o OBS está pegando)
#    -> install/Fix-OBSAudio.ps1 já faz essa enumeração
```

IDs do `BTHUSB` usados neste relatório:

| ID | Significado | Observado |
|---|---|---|
| 18 | Windows não consegue guardar chaves de link no adaptador | a cada reinício do rádio |
| 31 | sem buffer para filtragem de anúncios BLE | a cada reinício |
| 34 | sem estado de controlador de baixo consumo para modo periférico | a cada reinício |
| 3 | *timeout* de comando HCI | 60 s após o id 16 |
| 16 | falha de autenticação mútua com o fone | logo após o reinício do rádio |
