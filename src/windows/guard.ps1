$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$src = $env:LP_SCRIPT
function Block($m) { "BLOCKED (read-only mode): $m"; exit 3 }

$tok = $null; $perr = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($src, [ref]$tok, [ref]$perr)
if ($perr.Count) { "Syntax error: $($perr[0].Message) (line $($perr[0].Extent.StartLineNumber))"; exit 2 }

$cmdlets = @(
 'Get-Process','Get-Service','Get-CimInstance','Get-ComputerInfo','Get-HotFix','Get-WinEvent','Get-EventLog',
 'Get-ChildItem','Get-Item','Get-ItemProperty','Get-ItemPropertyValue','Test-Path','Get-PSDrive',
 'Get-PhysicalDisk','Get-Disk','Get-Partition','Get-Volume','Get-StorageReliabilityCounter',
 'Get-NetAdapter','Get-NetAdapterStatistics','Get-NetIPAddress','Get-NetIPConfiguration','Get-NetIPInterface','Get-NetRoute','Get-NetTCPConnection','Get-NetUDPEndpoint',
 'Get-DnsClientServerAddress','Get-NetFirewallProfile','Get-NetConnectionProfile',
 'Get-ScheduledTask','Get-ScheduledTaskInfo','Get-PnpDevice','Get-PnpDeviceProperty','Get-AppxPackage','Get-MpComputerStatus','Get-BitLockerVolume','Get-Tpm','Get-Counter',
 'Get-Date','Get-Member','Get-Variable','Get-Culture','Get-TimeZone','Get-Uptime',
 'Where-Object','ForEach-Object','Select-Object','Sort-Object','Group-Object','Measure-Object','Compare-Object',
 'Format-Table','Format-List','Format-Wide','Out-String','ConvertTo-Json','ConvertTo-Csv','ConvertFrom-Json','Join-Path','Split-Path','Write-Output'
) | ForEach-Object { $_.ToLower() }
$cmdset = @{}; foreach ($c in $cmdlets) { $cmdset[$c] = $true }

# External programs: fixed arguments only. Anything else (and any variable argument) is blocked.
$exe = @{
 'ipconfig' = @('/all','/displaydns'); 'netstat' = @('-a','-n','-o','-e','-s','-r','-ano','-an','-abno'); 'arp' = @('-a'); 'route' = @('print')
 'systeminfo' = @(); 'tasklist' = @('/v','/svc','/apps'); 'driverquery' = @('/v','/si'); 'whoami' = @('/all','/groups','/priv','/user'); 'hostname' = @(); 'getmac' = @('/v'); 'tzutil' = @('/g')
}
$exeFirst = @{
 'reg' = @('query'); 'sc' = @('query','queryex','qc','qdescription','qfailure'); 'schtasks' = @('/query')
 'wevtutil' = @('qe','el','gl'); 'powercfg' = @('/list','/query','/a','/requests','/availablesleepstates','/getactivescheme')
}
$remoteArg = '^[/-](s|u|p|r|ru|rp)(:|$)'
$netshBad = 'set','add','delete','del','reset','import','export','dump','start','stop','connect','disconnect','trace','exec','advfirewall','wlan-set'

$methods = @('tostring','trim','trimstart','trimend','split','replace','substring','startswith','endswith','contains','tolower','toupper','indexof','lastindexof','padleft','padright','format','join',
 'round','floor','ceiling','abs','min','max','pow','sqrt','parse','tryparse','toint32','toint64','todouble','equals','compare','isnullorempty','isnullorwhitespace','match','matches','ismatch',
 'adddays','addhours','addminutes','addseconds','where','foreach','containskey','tolist','toarray','new','isnullorwhitespace','totaldays','compareto')
$types = @('int','int32','int64','long','uint32','uint64','double','decimal','float','single','bool','boolean','string','char','byte','datetime','timespan','regex','math','convert','array','hashtable',
 'object','pscustomobject','ordered','version','ipaddress','psobject','void','system.math','system.convert','system.datetime','system.timespan','system.string','system.version','system.net.ipaddress',
 'system.text.regularexpressions.regex','text.regularexpressions.regex')
$safeEnv = @('computername','username','userprofile','temp','tmp','windir','systemroot','systemdrive','programfiles','programfiles(x86)','programdata','appdata','localappdata','os','processor_architecture','number_of_processors','homedrive','homepath','userdomain','path','public')
$noRemote = @('computername','cimsession','session','credential','authentication','server','proxyserver')
$pathBad = @('\.ssh','\.aws','\.gnupg','\.kube','\.azure','\.claude','\.codex','\.git\\config','credential','secret','token','password','passwd','id_rsa','id_ed25519','\.pem','\.key','\.pfx','\.kdbx','\.env','auth\.json','cookies','login data','web data','user data','mozilla\\firefox','litepilot\\','\\sam$','\\security$','ntds')

function CheckType($tn) {
  $n = $tn.FullName.ToLower() -replace '\[\]$',''
  if ($types -notcontains $n) { Block "the type [$($tn.FullName)] is not allowed" }
}

foreach ($n in $ast.FindAll({ $true }, $true)) {
  $t = $n.GetType().Name
  if ($t -in 'FunctionDefinitionAst','TypeDefinitionAst','ConfigurationDefinitionAst','UsingStatementAst','DynamicKeywordStatementAst','TrapStatementAst') { Block "$t is not allowed" }

  if ($t -eq 'TypeExpressionAst' -or $t -eq 'TypeConstraintAst') { CheckType $n.TypeName }

  if ($t -eq 'FileRedirectionAst' -and $n.Location.Extent.Text -ne '$null') { Block 'redirecting output to a file is not allowed' }

  if ($t -eq 'AssignmentStatementAst') {
    $l = $n.Left
    while ($l -is [System.Management.Automation.Language.ConvertExpressionAst]) { $l = $l.Child }
    if ($l -is [System.Management.Automation.Language.IndexExpressionAst]) { $l = $l.Target }
    if ($l -isnot [System.Management.Automation.Language.VariableExpressionAst]) { Block 'only plain variables can be assigned (no changing properties of objects)' }
  }
  if ($t -eq 'UnaryExpressionAst' -and $n.TokenKind -in 'PlusPlus','MinusMinus','PostfixPlusPlus','PostfixMinusMinus' -and $n.Child -isnot [System.Management.Automation.Language.VariableExpressionAst]) { Block '++/-- only on plain variables' }

  if ($t -eq 'VariableExpressionAst') {
    $p = $n.VariablePath.UserPath
    if ($p -match '^(?i)env:(.*)$') { if ($safeEnv -notcontains $Matches[1].ToLower()) { Block "environment variable $($Matches[1]) is not allowed" } }
    elseif ($p.Contains(':') -and $p -notmatch '^(?i)(script|global|local|private):') { Block "variable drive '$p' is not allowed" }
  }

  if ($t -eq 'StringConstantExpressionAst' -or $t -eq 'ExpandableStringExpressionAst') {
    $v = if ($t -eq 'StringConstantExpressionAst') { $n.Value } else { $n.Value }
    if ($v -match '^\s*\\\\' -or $v -match '^\s*//') { Block 'network (UNC) paths are not allowed' }
    if ($v -match '(?i)(^|[^a-z])env:' -and $n.Parent -isnot [System.Management.Automation.Language.VariableExpressionAst]) { Block 'listing environment variables is not allowed' }
    if ($v -match '[\\/:]') { foreach ($b in $pathBad) { if ($v -match "(?i)$b") { Block 'that location looks private (keys, tokens, passwords, browser data)' } } }
  }

  if ($t -eq 'InvokeMemberExpressionAst') {
    if ($n.Member -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) { Block 'dynamic method names are not allowed' }
    if ($methods -notcontains $n.Member.Value.ToLower()) { Block "the method .$($n.Member.Value)() is not allowed" }
  }

  if ($t -eq 'CommandAst') {
    if ($n.InvocationOperator -ne 'Unknown') { Block 'the & and . operators are not allowed' }
    $e0 = $n.CommandElements[0]
    if ($e0 -isnot [System.Management.Automation.Language.StringConstantExpressionAst]) { Block 'commands must be written by name' }
    $name = $e0.Value
    if ($name -match '[\\/]' -or [WildcardPattern]::ContainsWildcardCharacters($name)) { Block "command '$name' is not allowed" }
    $al = Get-Alias -Name $name -ErrorAction SilentlyContinue
    $real = if ($al) { [string]$al.Definition } else { $name }
    if ([string]::IsNullOrEmpty($real)) { Block "command '$name' could not be resolved" }
    $lc = $real.ToLower()
    $args2 = @($n.CommandElements | Select-Object -Skip 1)
    if ($cmdset.ContainsKey($lc)) {
      foreach ($el in $args2) {
        if ($el -is [System.Management.Automation.Language.CommandParameterAst]) {
          $pn = $el.ParameterName.ToLower()
          if ($noRemote -contains $pn) { Block "-$($el.ParameterName) (remote or credentialed access) is not allowed" }
          if ($lc -eq 'foreach-object' -and 'membername'.StartsWith($pn)) { Block 'ForEach-Object -MemberName is not allowed' }
        }
        elseif ($lc -eq 'foreach-object' -and $el -is [System.Management.Automation.Language.StringConstantExpressionAst]) { Block 'ForEach-Object takes a { script block } only' }
      }
    }
    else {
      $base = $lc -replace '\.exe$',''
      $sa = @($args2 | ForEach-Object { if ($_ -is [System.Management.Automation.Language.StringConstantExpressionAst] -or $_ -is [System.Management.Automation.Language.CommandParameterAst] -or $_ -is [System.Management.Automation.Language.ConstantExpressionAst]) { $_.Extent.Text.Trim('"',"'").ToLower() } else { $null } })
      foreach ($x in $sa) { if ($x -and $x -match $remoteArg) { Block 'remote or credentialed arguments are not allowed' } }
      if ($exe.ContainsKey($base)) {
        $allow = $exe[$base]
        foreach ($x in $sa) { if ($null -eq $x -or $allow -notcontains $x) { Block "$base only allows: $($allow -join ' ')" } }
      }
      elseif ($exeFirst.ContainsKey($base)) {
        if ($sa.Count -lt 1 -or $null -eq $sa[0] -or $exeFirst[$base] -notcontains $sa[0]) { Block "$base only allows: $($exeFirst[$base] -join ' ')" }
      }
      elseif ($base -eq 'netsh') {
        if ($sa -notcontains 'show') { Block 'netsh is allowed only for "show" commands' }
        foreach ($x in $sa) { if ($null -eq $x -or $netshBad -contains $x -or $x -like 'key=*') { Block 'that netsh command is not allowed' } }
      }
      else { Block "'$name' is not on the read-only list" }
    }
  }
}

# Second lock: even if something slipped past the checks above, PowerShell's constrained language refuses unsafe .NET calls.
$ExecutionContext.SessionState.LanguageMode = 'ConstrainedLanguage'
try { $out = Invoke-Expression $src *>&1 | Out-String -Width 200 } catch { $out = "ERROR: $($_.Exception.Message)" }
$out = $out -replace '(?i)(password|passwd|pwd|secret|token|api[_-]?key|authorization|bearer)(["''\s:=]+)[^\s"'']{4,}','$1$2***'
if ($out.Length -gt 12000) { $out = $out.Substring(0, 12000) + "`n...(cut)" }
$out
