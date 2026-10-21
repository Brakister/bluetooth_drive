# BTGuard — supervisor do rádio Bluetooth USB (CSR Nanosira)

**Problema:** ao abrir o OBS (ou qualquer app que abra o microfone *Hands-Free*
do fone Bluetooth), o dongle **CSR Nanosira** clone reinicia o controlador e o
fone **H02D Pro Anc** fica desconectado — com a pior parte: **é preciso
reiniciar o PC** para o Bluetooth voltar.

**Solução:** um supervisor nativo (Rust) que

1. **endurece o driver** para o rádio nunca mais suspender (a suspensão USB é o
   que reinicia o controlador e apaga a chave de link da RAM do dongle), e
2. **vigia o rádio em tempo real** (push, via `EvtSubscribe`) e **reinicia só o
   dongle** se ele travar — **sem reiniciar o PC, sem pedir nada ao usuário**.

A análise completa (engenharia reversa de `BTHUSB.SYS` e `bthport.SYS`, linha do
tempo do Event Log e do log do OBS) está em **[RELATORIO-BT.md](RELATORIO-BT.md)**.

---

## Sumário

- [O que acontece](#o-que-acontece)
- [O que o BTGuard faz](#o-que-o-btguard-faz)
- [Instalação](#instalação)
- [Comandos](#comandos)
- [Arquivos](#arquivos)
- [Como o vigia é "push" (sem polling)](#como-o-vigia-é-push-sem-polling)
- [Recuperação automática](#recuperação-automática)
- [Desinstalação](#desinstalação)
- [Limitação conhecida](#limitação-conhecida)

---

## O que acontece

O dongle USB `USB\VID_0A12&PID_0001` (clone CSR Nanosira) usa o driver
genérico da Microsoft `BthUsb` (`bth.inf`). Esse driver lê duas chaves do
registro do rádio (descoberto por engenharia reversa em `BTHUSB!FUN_14001882c`):

| Chave | Efeito |
|---|---|
| `ForceSelectiveSuspend = 1` | **força** o *selective suspend* USB (liga os bits `0x60` da capability USB) |
| `SelectiveSuspendSupported = 0` | declara o rádio **sem** suporte a suspensão → o driver **desabilita** o suspend seletivo |

No estado de fábrica o dongle era **declarado como capaz de suspender** e na
verdade dormia: `DeviceSelectiveSuspended = 1` no registro do rádio.

Quando o dongle dorme e acorda, o firmware do clone **reinicia o controlador
Bluetooth**. Cada reinício dispara a sequência `BTHUSB` ids 18 → 34 → 31
(= o rádio inicializou de novo) e — o que realmente importa — **derruba a
chave de link**.

Onde a chave de link vive? Três fontes de evidência, todas convergem:

| Onde? | Verificação | Resultado |
|---|---|---|
| **no dongle** | evento BTHUSB id 18: *"O Windows não pode armazenar códigos de autenticação Bluetooth (chaves de link) no adaptador local"* | **não** — o rádio não tem onde guardar |
| **no registro** | varredura binária de todo `HKLM\...\Services\BTHPORT` + `Control\Bluetooth` (procura-se um blob de 16 bytes) | **não** — só existe o marcador `Store Link Key = 1` (ver [RE](RELATORIO-BT.md#re-de-bthportsys--store-link-key)) |
| **na RAM do host** | o par é revalidado a cada conexão; a chave só existe enquanto o processo do Windows está vivo | **sim** |

Ou seja: **a chave de link é volátil**. Perde-se quando o rádio reinicia ou
quando o PC reinicia — e é por isso que o Bluetooth ficava morto até o
reboot. Nada disso é culpa do OBS: o OBS só acordava o rádio.

A partir daí o fone tenta reconectar e falha na autenticação mútua
(**BTHUSB id 16**: *"Falha na autenticação mútua ... (ee:e7:a9:aa:a8:af)"*),
seguido de **id 3** (*timeout* de comando HCI) 60 s depois. O Bluetooth fica
morto até um reinício do PC.

O OBS era o gatilho mais frequente porque ele abria o endpoint **HFP**
(*Hands-Free*, 16 kHz) do fone: a troca de perfil A2DP → HFP acorda o rádio.

---

## O que o BTGuard faz

### 1. Endurecimento do driver (registro, com backup)

Aplica nas quatro instâncias do rádio em
`HKLM\SYSTEM\CurrentControlSet\Enum\USB\VID_0A12&PID_0001\<instância>\Device Parameters`:

- `SelectiveSuspendSupported = 0` → **o rádio nunca mais dorme** (é a correção
  que impede o reinício do controlador);
- `EnhancedPowerManagementEnabled = 0` → desliga o gerenciamento de energia
  estendido do barramento USB.

O estado original de cada valor fica em
`%ProgramData%\BTGuard\backup-hardening.txt` e é revertido pelo
`btguard unharden`.

> **Por que o rádio precisa ser reiniciado depois do endurecimento?**
> O driver lê essas chaves no `PnpStartDevice`. Sem reiniciar o *devnode*,
> o valor novo fica só no registro e o driver continua com a decisão antiga.
> Por isso o `install` reinicia o rádio logo em seguida (e só o dongle — a
> sessão Bluetooth cai por ~2 s e volta).

### 2. Corrigir a captura de áudio do OBS

`install/Fix-OBSAudio.ps1` aponta a fonte **Mic/Aux** do OBS para um
microfone **não-Bluetooth** (por padrão o microfone da webcam) em vez de
`"default"`. Com um fone conectado, o Windows promove o endpoint HFP do fone a
microfone padrão — e abrir esse endpoint é o que dispara a troca de perfil e o
reinício do rádio. O script faz uma edição **cirúrgica** do JSON do OBS
(não resserializa o arquivo) e grava um backup.

### 3. Vigia com recuperação automática

O `btguard watch` roda como `SYSTEM` (tarefa agendada `BTGuard`, disparada no
boot +30 s, prioridade 7, `RestartOnFailure`) e:

- assina o log `System` com **`EvtSubscribe` + callback** (push — sem polling);
- quando o rádio reinicia (id 18), dá 12 s para o fone reconectar e, se ele não
  voltar, decide o que fazer conforme a causa real:
  - **rádio travado** (`radio_healthy() == None`) → **reinicia só o dongle**
    (`CM_Disable_DevNode` / `CM_Enable_DevNode`);
  - **pareamento velho** (houve falha de autenticação, id 16) → **refaz o
    pareamento** (a chave de link é volátil: só o bonding devolve o fone);
  - **fone simplesmente desligado** → não faz nada (ninguém reinicia nada);
- **nunca entra em laço**: intervalo mínimo de 3 min entre reparos, recuo
  exponencial até 30 min, teto de 6 tentativas seguidas, e uma janela em que os
  eventos gerados pelo *próprio* reparo são ignorados;
- tudo é registrado em `%ProgramData%\BTGuard\btguard.log`.

### 4. Parear / re-parear sem tocar no Windows à mão

`btguard pair` usa a API do Windows (`bluetoothapis.dll` + `bthprops.cpl` — o
mesmo caminho do *Settings*), sem automação de interface:

1. varre em modo pareamento (`BluetoothFindFirstDevice` com `fIssueInquiry`);
2. emparelha com SSP (`BluetoothAuthenticateDevice`, *Just Works*);
3. religa os serviços de áudio (`BluetoothSetServiceState`): **Audio Sink**
   (A2DP) e **Handsfree** (HFP) — sem eles o fone fica pareado mas não aparece
   como saída de áudio nem como microfone.

> ⚠️ **Pegadinha da API:** `BluetoothSetServiceState` com `dwServiceFlags = 0`
> significa **DESLIGAR** o serviço, não "consultar". Usar isso como se fosse
> consulta desliga silenciosamente o A2DP/HFP e o fone cai em ~3 s. O
> `btguard` lê o estado real no registro.

---

## Instalação

Requisitos: Windows 10 2004+ / Windows 11, PowerShell 5.1+ e Rights
Administrador. Para compilar a partir do fonte: Rust (`rustup`) e as
*Build Tools* do Visual Studio (ou o *Build Tools* do Rust).

```powershell
# clone
git clone https://github.com/Brakister/bluetooth_drive.git
cd bluetooth_drive

# instala (compila se preciso, pede UAC, endurece o driver, corrige o OBS
# e registra a tarefa de vigilancia)
powershell -ExecutionPolicy Bypass -File install\Install-BTGuard.ps1
```

Se já tiver o binário compilado:

```powershell
powershell -ExecutionPolicy Bypass -File install\Install-BTGuard.ps1 -Exe .\btguard.exe
```

Opções:

| Opção | Efeito |
|---|---|
| `-SkipObsFix` | não mexe na configuração do OBS |
| `-Exe <caminho>` | usa um binário pronto em vez de compilar |
| `-NoPause` | não espera o `ENTER` no final |

### O que o instalador faz

1. localiza/compila o `btguard.exe`;
2. roda `btguard install --elevated`, que
   - copia o executável para `%ProgramData%\BTGuard\btguard.exe`,
   - aplica o endurecimento (com backup),
   - **reinicia o rádio** para o endurecimento entrar em vigor,
   - registra a tarefa agendada `BTGuard` (via o módulo `ScheduledTasks` do
     PowerShell — o `schtasks /Create /XML` do Windows **rejeita**
     `LogonType=ServiceAccount`, bug conhecido),
   - inicia a vigilância na hora;
3. roda `install/Fix-OBSAudio.ps1`.

---

## Comandos

```
btguard status      # diagnóstico: rádios, dispositivos, estado do endurecimento, últimos eventos
btguard diag        # diagnóstico completo (devices via Bluetooth API + variantes de log)
btguard harden      # aplica o endurecimento (precisa de admin)
btguard unharden    # reverte o endurecimento a partir do backup
btguard recover     # reinicia o rádio agora (devolve o Bluetooth sem reiniciar o PC)
btguard pair [nome] # emparelha um dispositivo (ex.: btguard pair h02d) — API do Windows
btguard unpair [end]# remove o pareamento de um dispositivo
btguard audio [end] # religa os serviços de áudio (A2DP/HFP) de um dispositivo pareado
btguard scan        # varredura RF: mostra quem está anunciando presença (modo pareamento)
btguard watch       # vigia contínua (é o que a tarefa agendada executa)
btguard install     # instala (exe + endurecimento + tarefa)
btguard uninstall   # remove a tarefa, reverte o registro e apaga o exe
```

Os comandos que tocam no registro ou no rádio pedem elevação automaticamente
(`Start-Process -Verb RunAs`).

---

## Arquivos

```
BTGuard/
├─ Cargo.toml
├─ src/
│  ├─ main.rs      # CLI, auto-elevação, install/uninstall, registro da tarefa
│  ├─ guard.rs     # vigia push (EvtSubscribe + callback), reparo do rádio, re-pareamento
│  ├─ harden.rs    # endurecimento do driver (com backup/reversão)
│  ├─ pair.rs      # pareamento via API do Windows (bluetoothapis/bthprops) e serviços de áudio
│  ├─ pnp.rs       # descoberta do rádio (CM_Locate_DevNodeW) e dispositivos (Bluetooth API)
│  ├─ reg.rs       # wrappers de registro (HKLM, DWORD, delete)
│  └─ util.rs      # log com rotação, hora local, wide strings
├─ install/
│  ├─ Install-BTGuard.ps1   # instalador (auto-elevação, build, OBS, tarefa)
│  ├─ Uninstall-BTGuard.ps1 # desinstalador (reverte registro e OBS)
│  └─ Fix-OBSAudio.ps1      # corrige a captura de áudio do OBS
├─ README.md
└─ RELATORIO-BT.md
```

---

## Como o vigia é "push" (sem polling)

O requisito era **zero impacto de CPU/latência**. O vigia:

1. assina o canal `System` com `EvtSubscribe` e um *callback*
   (`EvtSubscribeActionDeliver`);
2. o callback só enfileira o evento e chama `SetEvent` num evento manual;
3. a thread principal dorme em `WaitForSingleObject` e só acorda quando há
   evento novo.

Nenhuma varredura periódica. (Detalhe: o Windows recusa — *erro 87* — combinar
um *handle* de evento de sinal com um *callback*; o vigia então reassina sem o
*handle* e o próprio callback faz o `SetEvent`. Só se a assinatura inteira
falhar é que ele cai para um *polling* de 3 s.)

---

## Recuperação automática

O vigia decide a ação pela **causa real**, não pelo sintoma:

| Situação detectada | Ação | Porquê |
|---|---|---|
| Rádio reiniciou (id 18) e o fone volta em ~60 s | **nada** | foi só uma reinicialização |
| Rádio reiniciou e **não volta** (id 3 = *timeout* HCI) | **reinicia só o dongle** | o rádio travou de verdade |
| Rádio responde, mas o fone não volta e houve **id 16** | **refaz o pareamento** | a chave de link é volátil; só o bonding resolve |
| Fone está no registro mas a pilha nem o conhece mais | **refaz o pareamento** | o host perdeu a chave (estado pós-reboot) |
| Rádio responde, fone não volta, **sem** id 16 | **nada** | o fone está desligado/sem bateria |
| Pareamento perdido de vez | id 10 (*chave removida*) | o vigia re-pareia no próximo ciclo |

Em todos os caminhos: **sem reiniciar o PC**. O caso do re-pareamento exige
apenas que o **fone esteja ligado**; se estiver desligado, o vigia tenta de
novo mais tarde com espera crescente — e **nunca destrói** o pareamento
antigo: o registro é exportado antes e restaurado se o re-pareamento falhar.

### E depois de reiniciar o PC?

A chave de link é **volátil** (ver [O que acontece](#o-que-acontece)): ela
morre com o processo do Windows, não fica em disco. Então, depois de um
reinício do PC, o fone precisa ser **pareado de novo** — mas o BTGuard faz
isso sozinho:

1. o rádio reinicializa no boot (id 18) e o fone falha a autenticação
   (id 16) — os dois eventos acontecem **antes** da tarefa subir, então o vigia
   os recupera numa leitura única do log na largada;
2. 90 s depois, o vigia confere: o rádio está saudável e o fone não voltou →
   o problema é a chave de link → **re-pareia** (varredura → SSP *Just Works*
   → religa A2DP/HFP);
3. o fone volta a funcionar sozinho. **Nenhuma ação do usuário, nenhum
   reinício de PC.**

Requisito: o fone precisa estar **ligado** e **descobrível** (modo pareamento).
A maioria dos fones entra em modo pareamento sozinha depois de uma
autenticação falha — e o `re_pair` do BTGuard varre o ar de qualquer forma.

> Nota: se o fone estiver desligado no boot, o vigia espera — ele só age
> quando o fone *anuncia* sua presença.

---

## Desinstalação

```powershell
powershell -ExecutionPolicy Bypass -File install\Uninstall-BTGuard.ps1
```

Remove a tarefa agendada, reverte o endurecimento (a partir do backup),
restaura a configuração original do OBS e apaga o executável instalado. O log e
o backup são preservados em `%ProgramData%\BTGuard`.

---

## Limitação conhecida

Este repositório conserta o **comportamento do software**. O dongle CSR
Nanosira **clone** continua sendo hardware ruim: o evento BTHUSB id 18 é
emitido em **todo** reinício do rádio e diz que o Windows não consegue guardar
a chave de link **no adaptador** — ou seja, ela vive só na RAM do dongle. Se o
firmware do clone reiniciar fora do controle do Windows (brown-out, dongle
defeituoso), o pareamento é perdido de novo e o fone precisa ser pareado
novamente.

**Recomendação de hardware:** trocar o clone por um **Intel AX (AX210/AX211)**,
um **Realtek RTL8761B**, ou um **CSR Nanosira genuíno** — todos com
armazenamento persistente de chaves de link. Detalhes em
[RELATORIO-BT.md](RELATORIO-BT.md#recomendação-de-hardware).
