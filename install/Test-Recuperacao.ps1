# Verifica se a recuperacao automatica do BTGuard funcionou.
#
# Uso (depois de reiniciar o PC, com o fone ligado):
#   powershell -ExecutionPolicy Bypass -File install\Test-Recuperacao.ps1
#
# Saida: um veredito por item + o trecho relevante do log do vigia.

$ErrorActionPreference = 'Continue'

$exe = Join-Path $env:ProgramData 'BTGuard\btguard.exe'
$log = Join-Path $env:ProgramData 'BTGuard\btguard.log'
$addr = 'eee7a9aaa8af'

function Say($ok, $txt) {
    $mark = if ($ok) { '[ OK ]' } else { '[FALHA]' }
    $color = if ($ok) { 'Green' } else { 'Red' }
    Write-Host "$mark $txt" -ForegroundColor $color
}

Write-Host ''
Write-Host '== BTGuard - verificacao de recuperacao ==' -ForegroundColor Cyan
Write-Host ("hora: " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
Write-Host ''

# 1) o vigia esta rodando?
$proc = Get-Process btguard -ErrorAction SilentlyContinue
Say ($null -ne $proc) ("vigia rodando" + $(if ($proc) { " (pid $($proc.Id))" } else { '' }))

# 2) a tarefa agendada esta registrada?
#    (consultar uma tarefa que roda como SYSTEM exige admin; se faltar, testa
#     o arquivo da task, que nao exige privilegio)
$taskOk = $false
try {
    $t = Get-ScheduledTask -TaskName BTGuard -ErrorAction Stop
    $taskOk = ($null -ne $t)
} catch {
    $taskOk = Test-Path 'C:\Windows\System32\Tasks\BTGuard' -ErrorAction SilentlyContinue
    if (-not $taskOk) { $taskOk = $null -ne (Get-Process btguard -ErrorAction SilentlyContinue) }
}
Say $taskOk 'tarefa agendada BTGuard registrada'

# 3) o fone esta conectado e autenticado?
$diag = & $exe diag 2>&1 | Out-String
$conectado = $diag -match ([regex]::Escape($addr) + '.*conectado=true')
$autenticado = $diag -match ([regex]::Escape($addr) + '.*autenticado=true')
Say $conectado 'fone conectado (Bluetooth API)'
Say $autenticado 'fone autenticado (chave de link valida)'

# 4) o endurecimento continua aplicado?
$status = & $exe status 2>&1 | Out-String
Say ($status -match 'SelectiveSuspendSupported=Some\(0\)') 'endurecimento aplicado (SelectiveSuspendSupported=0)'

# 5) houve reinicializacao espontanea do radio?
#    O id 18 tambem aparece quando o PROPRIO vigia reinicia o dongle, entao
#    os reinicios do watchdog sao descartados (correlacionando com o log).
$logLines = @()
if (Test-Path $log) { $logLines = Get-Content $log }
$reiniciosDoVigia = @($logLines | Where-Object { $_ -match '^\s*(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}).*reiniciando devnode' } |
    ForEach-Object { [datetime]::ParseExact($Matches[1], 'yyyy-MM-dd HH:mm:ss', $null) })

$ev = Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'BTHUSB'; Id = 18 } -MaxEvents 40 -ErrorAction SilentlyContinue
# Referencia: a ultima vez que o endurecimento foi aplicado (no log).
$desde = $null
$logLines | Where-Object { $_ -match 'endurecimento aplicado' } | ForEach-Object {
    if ($_ -match '^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})') {
        $desde = [datetime]::ParseExact($Matches[1], 'yyyy-MM-dd HH:mm:ss', $null)
    }
}
$espontaneos = @($ev | Where-Object {
    $t = $_.TimeCreated
    if ($desde -and $t -lt $desde) { return $false }
    -not ($reiniciosDoVigia | Where-Object { [math]::Abs(($_ - $t).TotalSeconds) -le 15 })
})
Say ($espontaneos.Count -eq 0) ("reinicios espontaneos do radio desde o endurecimento (" + $(if ($desde) { $desde.ToString('dd/MM HH:mm') } else { '?' }) + "): " + $espontaneos.Count)
if ($espontaneos.Count -gt 0) {
    $espontaneos | Select-Object -First 5 | ForEach-Object { Write-Host ("        " + $_.TimeCreated.ToString('dd/MM HH:mm:ss')) -ForegroundColor DarkGray }
}

Write-Host ''
Write-Host '== trecho do log do vigia ==' -ForegroundColor Cyan
if (Test-Path $log) {
    Get-Content $log -Tail 25 | ForEach-Object { Write-Host ("  " + $_) -ForegroundColor DarkGray }
} else {
    Write-Host '  (log inexistente)' -ForegroundColor DarkGray
}

Write-Host ''
Write-Host '== como foi a recuperacao ==' -ForegroundColor Cyan
if (Test-Path $log) {
    $txt = Get-Content $log -Raw
    foreach ($pat in @('re-pareamento de .* concluido', 'pareamento invalido .* refazendo o pareamento',
            'reconexao falhou', 'registro antigo restaurado', 'pareando ', 'vinculado com sucesso')) {
        $hits = Select-String -InputObject $txt -Pattern $pat -AllMatches
        if ($hits) {
            foreach ($m in $hits.Matches) { Write-Host ("  * " + $m.Value) -ForegroundColor Yellow }
        }
    }
}
Write-Host ''
