//! The diagnostic tool server the CLI starts only for messages that need it (LitePilot.exe --system-mcp lang,proc).
//! Each `what` runs one fixed read-only query: a PowerShell script built into the app, a fixed program with fixed
//! arguments, or plain Rust. The model's values are checked first (core::diag) and reach PowerShell only as
//! environment variables, never as command text. Every result is shaped (pruned, redacted, capped, wrapped) before
//! it leaves, and every call is noted in diag-audit.log (tool, what, parameters, size; never the output).
use crate::core::diag::{self as d, BIG_CAP, CAP};
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const SERVER: &str = "lp_system";
const SECS: u64 = 10;
const MAX_READ: u64 = 64 * 1024;
/// About three rounds of tool calls per message; past that the model is told to answer with what it has.
const MAX_CALLS: u32 = 10;
/// Written by the app before each message that has tools, so the call count starts again.
pub const TURN_FILE: &str = "diag-turn";

/// Around every script: UTF-8 out, stop on errors, `$n` rows, `L` for a message's first line, compact JSON,
/// and "requires_elevation" instead of an access error.
const PRE: &str = "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[Text.Encoding]::UTF8;\
$n=[int]$env:LP_N;function L($s){$x=(\"$s\" -split \"`r?`n\")[0];if($x.Length -gt 200){$x.Substring(0,200)}else{$x}};try{$r=@(& {";
const POST: &str = "});if($r.Count -eq 1){$r=$r[0]};ConvertTo-Json -InputObject $r -Compress -Depth 4}catch{$m=$_.Exception.Message;\
if($_.Exception -is [UnauthorizedAccessException] -or $m -match 'denied|elevat|administrator|privilege'){'{\"error\":\"requires_elevation\"}'}\
elseif($m -match 'No events were found'){'[]'}else{ConvertTo-Json -Compress @{error=$m}}}";

const INSTALLED: &str = r#"Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*','HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*' -EA 0|?{$_.DisplayName -and $_.DisplayName.IndexOf("$env:LP_A",[StringComparison]::OrdinalIgnoreCase) -ge 0}|sort DisplayName -Unique|select -f $n|%{@{name=$_.DisplayName;ver=$_.DisplayVersion;by=$_.Publisher}}"#;
const REG_READ: &str = r#"$o=[ordered]@{};(Get-ItemProperty -LiteralPath $env:LP_K).PSObject.Properties|?{$_.Name -notlike 'PS*' -and $_.Value -isnot [byte[]]}|select -f $n|%{$o[$_.Name]=$_.Value};$o"#;
/// The registry keys `reg` may read (core::diag::REG_KEYS; "uninstall" is the installed-programs query).
const REG: &[(&str, &str)] = &[
    ("run", r"HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"),
    ("run_user", r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"),
    ("winver", r"HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
    ("timezone", r"HKLM:\SYSTEM\CurrentControlSet\Control\TimeZoneInformation"),
    ("international", r"HKCU:\Control Panel\International"),
];

/// The one fixed query behind each PowerShell-backed tool/what.
fn script(tool: &str, what: &str) -> Option<&'static str> {
    Some(match (tool, what) {
        ("sys", "os") => r#"$o=Get-CimInstance Win32_OperatingSystem;$v=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion';@{os=$o.Caption;ver=$v.DisplayVersion;build="$($o.BuildNumber).$($v.UBR)";arch=$o.OSArchitecture;up_h=[int]((Get-Date)-$o.LastBootUpTime).TotalHours;since=$o.InstallDate.ToString('yyyy-MM-dd')}"#,
        ("sys", "hardware") => r#"$c=Get-CimInstance Win32_Processor|select -f 1;$s=Get-CimInstance Win32_ComputerSystem;$o=Get-CimInstance Win32_OperatingSystem;@{cpu=$c.Name.Trim();cores=$c.NumberOfCores;threads=$c.NumberOfLogicalProcessors;load_pct=$c.LoadPercentage;ram_gb=[math]::Round($s.TotalPhysicalMemory/1GB,1);free_gb=[math]::Round($o.FreePhysicalMemory/1MB,1);model="$($s.Manufacturer) $($s.Model)";gpu=@(Get-CimInstance Win32_VideoController|%{$_.Name});bios=(Get-CimInstance Win32_BIOS).SMBIOSBIOSVersion}"#,
        ("sys", "updates") => r#"Get-HotFix|sort InstalledOn -Desc|select -f $n|%{@{id=$_.HotFixID;type=$_.Description;on=$(if($_.InstalledOn){$_.InstalledOn.ToString('yyyy-MM-dd')})}}"#,
        ("sys", "pending_reboot") => r#"$k='HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\';@{servicing=Test-Path ($k+'Component Based Servicing\RebootPending');update=Test-Path ($k+'WindowsUpdate\Auto Update\RebootRequired');file_renames=$null -ne (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' -EA 0).PendingFileRenameOperations}"#,
        ("sys", "activation") => r#"Get-CimInstance SoftwareLicensingProduct -Filter "ApplicationID='55c92734-d682-4d71-983e-d6ec3f16059f' AND PartialProductKey IS NOT NULL"|%{@{name=$_.Name;status=@('unlicensed','licensed','oob_grace','oot_grace','non_genuine','notification','extended_grace')[$_.LicenseStatus];channel=$_.ProductKeyChannel}}"#,
        ("sys", "battery") => r#"$b=@(Get-CimInstance Win32_Battery);if(!$b){@{battery='none'}}else{$b|%{@{name=$_.Name;charge_pct=$_.EstimatedChargeRemaining;on_mains=$_.BatteryStatus -ne 1;mins_left=$(if($_.EstimatedRunTime -lt 71582788){$_.EstimatedRunTime})}}}"#,
        ("sys", "display") => r#"Get-CimInstance Win32_VideoController|%{@{gpu=$_.Name;res="$($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)";hz=$_.CurrentRefreshRate;driver=$_.DriverVersion;status=$_.Status}}"#,
        ("sys", "audio") => r#"@{devices=@(Get-CimInstance Win32_SoundDevice|%{"$($_.Name) [$($_.Status)]"});service="$((Get-Service Audiosrv).Status)"}"#,
        ("sys", "printers") => r#"Get-CimInstance Win32_Printer|select -f $n|%{@{name=$_.Name;default=$_.Default;offline=$_.WorkOffline;status=$_.PrinterStatus;port=$_.PortName}}"#,
        ("lang", "languages_keyboards") => r#"Get-WinUserLanguageList|%{@{tag=$_.LanguageTag;name=$_.LocalizedName;kb=@($_.InputMethodTips|%{$t=(Get-ItemProperty ('HKLM:\SYSTEM\CurrentControlSet\Control\Keyboard Layouts\'+($_ -split ':')[1]) -EA 0).'Layout Text';"$_ $t".Trim()})}}"#,
        ("lang", "locale") => r#"$c=Get-Culture;@{format=$c.Name;ui=(Get-UICulture).Name;system=(Get-WinSystemLocale).Name;geo_id=(Get-WinHomeLocation).HomeLocation;date=$c.DateTimeFormat.ShortDatePattern}"#,
        ("lang", "timezone") => r#"$t=Get-TimeZone;@{id=$t.Id;name=$t.DisplayName;dst=$t.SupportsDaylightSavingTime;now=(Get-Date).ToString('s')}"#,
        ("lang", "input_method") => r#"$b=Get-WinLanguageBarOption;@{default_override=(Get-WinDefaultInputMethodOverride).InputMethodTip;legacy_switch=$b.IsLegacySwitch;legacy_bar=$b.IsLegacyLanguageBar}"#,
        ("disk", "volumes") => r#"Get-Volume|?{$_.DriveLetter}|sort DriveLetter|%{@{drive="$($_.DriveLetter):";label=$_.FileSystemLabel;fs=$_.FileSystem;gb=[math]::Round($_.Size/1GB,1);free_gb=[math]::Round($_.SizeRemaining/1GB,1);health="$($_.HealthStatus)"}}"#,
        ("disk", "physical") => r#"Get-PhysicalDisk|%{@{name=$_.FriendlyName;media="$($_.MediaType)";bus="$($_.BusType)";gb=[math]::Round($_.Size/1GB);health="$($_.HealthStatus)";status="$($_.OperationalStatus)"}}"#,
        ("proc", "top") => r#"$a=@{};Get-Process|%{$a[$_.Id]=$_.CPU};[Threading.Thread]::Sleep(1000);$p=Get-Process;$c=[Environment]::ProcessorCount;@{by_ram=@($p|sort WS -Desc|select -f $n|%{@{name=$_.ProcessName;pid=$_.Id;ram_mb=[math]::Round($_.WS/1MB);cpu_s=[math]::Round($_.CPU)}});cpu_now_pct=@($p|%{[pscustomobject]@{n=$_.ProcessName;d=$_.CPU-$a[$_.Id]}}|sort d -Desc|select -f 5|%{"$($_.n) $([math]::Round(100*$_.d/$c))"})}"#,
        ("proc", "details") => r#"Get-Process|?{$_.ProcessName.IndexOf("$env:LP_A",[StringComparison]::OrdinalIgnoreCase) -ge 0}|select -f $n|%{@{name=$_.ProcessName;pid=$_.Id;ram_mb=[math]::Round($_.WS/1MB);cpu_s=[math]::Round($_.CPU);threads=$_.Threads.Count;handles=$_.HandleCount;started=$(try{$_.StartTime.ToString('s')}catch{});path=$(try{$_.Path}catch{});title=$_.MainWindowTitle}}"#,
        ("proc", "perf") => r#"$p=Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'";$m=Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory;$d=Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -Filter "Name='_Total'";@{cpu_pct=$p.PercentProcessorTime;ram_free_mb=$m.AvailableMBytes;commit_pct=$m.PercentCommittedBytesInUse;disk_busy_pct=$d.PercentDiskTime;disk_queue=$d.CurrentDiskQueueLength}"#,
        ("proc", "startup") => r#"Get-CimInstance Win32_StartupCommand|select -f $n|%{@{name=$_.Name;cmd=$_.Command;where=$_.Location}}"#,
        ("proc", "services") => r#"Get-Service|?{if($env:LP_A){($_.Name+' '+$_.DisplayName).IndexOf($env:LP_A,[StringComparison]::OrdinalIgnoreCase) -ge 0}else{$_.StartType -eq 'Automatic' -and $_.Status -ne 'Running'}}|select -f $n|%{@{name=$_.Name;shown=$_.DisplayName;status="$($_.Status)";start="$($_.StartType)"}}"#,
        ("proc", "port_owner") => r#"$p=[int]$env:LP_P;@(Get-NetTCPConnection -LocalPort $p -EA 0)+@(Get-NetUDPEndpoint -LocalPort $p -EA 0)|select -f $n|%{@{proto=$(if($_.State){'tcp'}else{'udp'});state="$($_.State)";local="$($_.LocalAddress):$($_.LocalPort)";remote=$(if($_.RemoteAddress){"$($_.RemoteAddress):$($_.RemotePort)"});pid=$_.OwningProcess;proc=(Get-Process -Id $_.OwningProcess -EA 0).ProcessName}}"#,
        ("net", "adapters") => r#"Get-NetAdapter|%{@{name=$_.Name;desc=$_.InterfaceDescription;status="$($_.Status)";speed=$_.LinkSpeed;mac=$_.MacAddress}}"#,
        ("net", "ip") => r#"Get-NetIPConfiguration|?{$_.NetAdapter.Status -eq 'Up'}|%{@{iface=$_.InterfaceAlias;ipv4=@($_.IPv4Address.IPAddress);ipv6=@($_.IPv6Address.IPAddress);gateway=@($_.IPv4DefaultGateway.NextHop);dns=@($_.DNSServer.ServerAddresses)}}"#,
        ("net", "ping") => r#"$g=[Net.NetworkInformation.Ping]::new();$r=@(1..3|%{try{$g.Send($env:LP_A,2000)}catch{$e=$_.Exception.GetBaseException().Message}});$ok=@($r|?{"$($_.Status)" -eq 'Success'});@{host=$env:LP_A;replies="$($ok.Count)/3";ms=@($ok|%{$_.RoundtripTime});ip="$($ok[0].Address)";status="$($r[-1].Status)";error=$e}"#,
        ("net", "connect") => r#"$c=[Net.Sockets.TcpClient]::new();$ok=$false;try{$ok=$c.ConnectAsync($env:LP_A,[int]$env:LP_P).Wait(4000) -and $c.Connected}catch{$e=$_.Exception.GetBaseException().Message};$c.Dispose();@{host=$env:LP_A;port=[int]$env:LP_P;open=$ok;error=$e}"#,
        ("net", "dns") => r#"Resolve-DnsName -Name $env:LP_A -DnsOnly|select -f $n|%{@{name=$_.Name;type="$($_.Type)";ip=$_.IPAddress;target=$_.NameHost;ttl=$_.TTL}}"#,
        ("net", "ports") => r#"Get-NetTCPConnection -State Listen|sort LocalPort -Unique|select -f $n|%{@{port=$_.LocalPort;addr=$_.LocalAddress;pid=$_.OwningProcess;proc=(Get-Process -Id $_.OwningProcess -EA 0).ProcessName}}"#,
        ("net", "proxy") => r#"$i=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings';@{on=$i.ProxyEnable -eq 1;server=$i.ProxyServer;pac=$i.AutoConfigURL;bypass=$i.ProxyOverride;env=$env:HTTPS_PROXY}"#,
        ("net", "firewall") => r#"Get-NetFirewallProfile|%{@{name=$_.Name;on="$($_.Enabled)";inbound="$($_.DefaultInboundAction)";outbound="$($_.DefaultOutboundAction)"}}"#,
        ("net", "routes") => r#"Get-NetRoute -AddressFamily IPv4|sort RouteMetric|select -f $n|%{@{dest=$_.DestinationPrefix;hop=$_.NextHop;iface=$_.InterfaceAlias;metric=$_.RouteMetric}}"#,
        ("events", "errors") => r#"Get-WinEvent -FilterHashtable @{LogName='System','Application';Level=1,2;StartTime=(Get-Date).AddDays(-7)} -MaxEvents $n|%{@{t=$_.TimeCreated.ToString('s');log=$_.LogName;src=$_.ProviderName;id=$_.Id;msg=(L $_.Message)}}"#,
        ("events", "crashes") => r#"Get-WinEvent -FilterHashtable @{LogName='Application';Id=1000,1002;StartTime=(Get-Date).AddDays(-14)} -MaxEvents $n|%{@{t=$_.TimeCreated.ToString('s');kind=$(if($_.Id -eq 1000){'crash'}else{'hang'});app=$_.Properties[0].Value;module=$(if($_.Id -eq 1000){$_.Properties[3].Value});code=$(if($_.Id -eq 1000){$_.Properties[6].Value})}}"#,
        ("events", "bsod") => r#"Get-WinEvent -FilterHashtable @{LogName='System';Id=41,1001,6008;StartTime=(Get-Date).AddDays(-30)} -MaxEvents $n|?{$_.Id -ne 1001 -or $_.ProviderName -like '*WER*'}|%{@{t=$_.TimeCreated.ToString('s');id=$_.Id;msg=(L $_.Message)}}"#,
        ("events", "reliability") => r#"$s=Get-CimInstance Win32_ReliabilityStabilityMetrics|sort TimeGenerated|select -l 1;@{index=[math]::Round($s.SystemStabilityIndex,1);recent=@(Get-CimInstance Win32_ReliabilityRecords|sort TimeGenerated -Desc|select -f $n|%{@{t=$_.TimeGenerated.ToString('s');src=$_.SourceName;app=$_.ProductName;msg=(L $_.Message)}})}"#,
        ("events", "update_failures") => r#"Get-WinEvent -FilterHashtable @{LogName='System';ProviderName='Microsoft-Windows-WindowsUpdateClient';Id=20;StartTime=(Get-Date).AddDays(-60)} -MaxEvents $n|%{@{t=$_.TimeCreated.ToString('s');msg=(L $_.Message)}}"#,
        ("sec", "defender") => r#"$m=Get-MpComputerStatus;@{av_on=$m.AntivirusEnabled;realtime=$m.RealTimeProtectionEnabled;sig_age_days=$m.AntivirusSignatureAge;quick_scan=$(if($m.QuickScanEndTime){$m.QuickScanEndTime.ToString('s')});tamper=$m.IsTamperProtected;products=@(Get-CimInstance -Namespace root\SecurityCenter2 AntiVirusProduct -EA 0|%{$_.displayName})}"#,
        ("sec", "uac_secureboot") => r#"$u=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System';@{uac=$u.EnableLUA -eq 1;admin_prompt=$u.ConsentPromptBehaviorAdmin;secure_boot=(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\SecureBoot\State' -EA 0).UEFISecureBootEnabled -eq 1}"#,
        ("sec", "bitlocker") => r#"Get-BitLockerVolume|%{@{drive=$_.MountPoint;status="$($_.VolumeStatus)";protection="$($_.ProtectionStatus)";pct=$_.EncryptionPercentage}}"#,
        ("sec", "users") => r#"Get-LocalUser|%{@{name=$_.Name;on=$_.Enabled;last_logon=$(if($_.LastLogon){$_.LastLogon.ToString('s')})}}"#,
        ("sec", "admins") => r#"Get-LocalGroupMember -SID S-1-5-32-544|%{@{name=$_.Name;type="$($_.ObjectClass)";source="$($_.PrincipalSource)"}}"#,
        ("apps", "installed") => INSTALLED,
        ("apps", "problem_devices") => r#"Get-CimInstance Win32_PnPEntity -Filter 'ConfigManagerErrorCode<>0'|select -f $n|%{@{name=$_.Name;kind=$_.PNPClass;code=$_.ConfigManagerErrorCode}}"#,
        ("apps", "default_apps") => r#"$o=[ordered]@{};$a='HKCU:\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\';$f='HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\';foreach($k in 'http','https','mailto'){$o[$k]=(Get-ItemProperty ($a+$k+'\UserChoice') -EA 0).ProgId};foreach($k in '.pdf','.html','.txt','.jpg','.mp4','.zip'){$o[$k]=(Get-ItemProperty ($f+$k+'\UserChoice') -EA 0).ProgId};$o"#,
        ("apps", "env_vars") => r#"$o=[ordered]@{};foreach($s in 'User','Machine'){$e=[Environment]::GetEnvironmentVariables($s);$h=[ordered]@{};$e.Keys|sort|?{$_.IndexOf("$env:LP_A",[StringComparison]::OrdinalIgnoreCase) -ge 0}|select -f $n|%{$h[$_]=$e[$_]};$o[$s]=$h};$o"#,
        _ => return None,
    })
}

/// The `ps` tool: a script the model wrote, run only if every part of it is on an allowlist. The text arrives in an
/// environment variable (never as command text), is parsed into a syntax tree, and is refused unless it uses only
/// read-only cmdlets (by resolved name, so aliases can't sneak past), plain variables, allowlisted types and methods,
/// and no redirection, call operator, function, dot-source or write. The run itself is in ConstrainedLanguage mode as
/// a second wall. No cmdlet that reads file contents is allowed (the `file` tool does that, inside three folders).
const PS_GUARD: &str = r##"$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[Text.Encoding]::UTF8
function J($m){ConvertTo-Json -Compress @{error=$m}}
try{
$wCmd='Get-Process','Get-Service','Get-CimInstance','Get-ChildItem','Get-Item','Get-ItemProperty','Get-ItemPropertyValue','Get-PSDrive','Get-Volume','Get-Disk','Get-Partition','Get-PhysicalDisk','Get-StoragePool','Get-StorageReliabilityCounter','Get-NetAdapter','Get-NetAdapterStatistics','Get-NetIPConfiguration','Get-NetIPAddress','Get-NetIPInterface','Get-NetRoute','Get-NetTCPConnection','Get-NetUDPEndpoint','Get-NetFirewallProfile','Get-NetFirewallRule','Get-NetConnectionProfile','Get-DnsClientServerAddress','Get-DnsClientCache','Get-DnsClient','Get-SmbShare','Get-SmbConnection','Get-HotFix','Get-WinEvent','Get-EventLog','Get-ComputerInfo','Get-Culture','Get-UICulture','Get-TimeZone','Get-WinUserLanguageList','Get-WinSystemLocale','Get-WinHomeLocation','Get-MpComputerStatus','Get-MpThreatDetection','Get-BitLockerVolume','Get-Tpm','Get-LocalUser','Get-LocalGroup','Get-LocalGroupMember','Get-ScheduledTask','Get-ScheduledTaskInfo','Get-PnpDevice','Get-PnpDeviceProperty','Get-AppxPackage','Get-WindowsCapability','Get-Printer','Get-PrintJob','Get-Counter','Get-Command','Get-Module','Get-Member','Get-FileHash','Get-Acl','Get-Date','Get-ExecutionPolicy','Get-Location','Get-Alias','Get-Uptime','Test-Path','Test-Connection','Test-NetConnection','Resolve-DnsName','Resolve-Path','Split-Path','Join-Path','Select-Object','Where-Object','ForEach-Object','Sort-Object','Group-Object','Measure-Object','Compare-Object','Format-Table','Format-List','Format-Wide','Out-String','Out-Null','ConvertTo-Json','ConvertFrom-Json','ConvertTo-Csv','Write-Output','New-TimeSpan'
$wExe='systeminfo','hostname','whoami','getmac','tasklist','driverquery','netstat','ping','tracert','nslookup'
$wType='math','string','int','int32','int64','long','double','single','float','decimal','bool','boolean','datetime','timespan','array','regex','text.regularexpressions.regex','convert','environment','version','char','byte','guid','uri','pscustomobject','ordered','hashtable'
$wMeth='tostring','trim','trimstart','trimend','split','replace','contains','startswith','endswith','substring','indexof','lastindexof','tolower','toupper','padleft','padright','round','floor','ceiling','abs','min','max','pow','sqrt','parse','join','format','isnullorempty','isnullorwhitespace','concat','toint32','toint64','todouble','match','ismatch','matches','containskey','add','adddays','addhours','addminutes','addmonths','addyears','where','foreach','getenvironmentvariable'
$wNode='ScriptBlockAst','NamedBlockAst','StatementBlockAst','PipelineAst','CommandAst','CommandParameterAst','CommandExpressionAst','StringConstantExpressionAst','ConstantExpressionAst','ExpandableStringExpressionAst','VariableExpressionAst','MemberExpressionAst','InvokeMemberExpressionAst','IndexExpressionAst','BinaryExpressionAst','UnaryExpressionAst','ParenExpressionAst','SubExpressionAst','ArrayExpressionAst','ArrayLiteralAst','HashtableAst','ScriptBlockExpressionAst','TypeExpressionAst','TypeConstraintAst','ConvertExpressionAst','AssignmentStatementAst','IfStatementAst','ForEachStatementAst','ForStatementAst','WhileStatementAst','TryStatementAst','CatchClauseAst','BreakStatementAst','ContinueStatementAst','MergingRedirectionAst'
$src=$env:LP_SCRIPT;$tk=$null;$er=$null
$ast=[Management.Automation.Language.Parser]::ParseInput($src,[ref]$tk,[ref]$er)
if($er.Count){J ('syntax error: '+$er[0].Message);exit}
$bad=$null
foreach($n in $ast.FindAll({param($x)$true},$true)){
  $t=$n.GetType().Name
  if($wNode -notcontains $t){$bad="refused: $t is not allowed (writes, redirection, functions and flow tricks are refused)";break}
  if($t -eq 'CommandAst'){
    $e=$n.CommandElements[0]
    if($n.InvocationOperator -ne 'Unknown' -or $e -isnot [Management.Automation.Language.StringConstantExpressionAst]){$bad='refused: a command must be written as a plain name, without & or .';break}
    $c=@(Get-Command -Name ([Management.Automation.WildcardPattern]::Escape($e.Value)) -ErrorAction SilentlyContinue)|select -First 1
    if($c -and $c.CommandType -eq 'Alias'){$c=@(Get-Command -Name ([Management.Automation.WildcardPattern]::Escape($c.Definition)) -ErrorAction SilentlyContinue)|select -First 1}
    $ok=$c -and ((($c.CommandType -eq 'Cmdlet' -or $c.CommandType -eq 'Function') -and $wCmd -contains $c.Name) -or ($c.CommandType -eq 'Application' -and $wExe -contains [IO.Path]::GetFileNameWithoutExtension($c.Name) -and $c.Source -like "$env:SystemRoot\System32\*"))
    if(-not $ok){$bad="refused: command '$($e.Value)' is not on the read-only list";break}
  }
  elseif($t -eq 'InvokeMemberExpressionAst'){
    if($n.Member -isnot [Management.Automation.Language.StringConstantExpressionAst] -or $wMeth -notcontains $n.Member.Value.ToLower()){$bad="refused: method '$($n.Member.Extent.Text)' is not on the read-only list";break}
    if($n.Static -and $n.Expression -isnot [Management.Automation.Language.TypeExpressionAst]){$bad='refused: static call on a non-type';break}
  }
  elseif($t -eq 'MemberExpressionAst'){
    if($n.Static -and $n.Expression -isnot [Management.Automation.Language.TypeExpressionAst]){$bad='refused: static member on a non-type';break}
  }
  elseif($t -eq 'TypeExpressionAst' -or $t -eq 'TypeConstraintAst'){
    $tn=($n.TypeName.Name.ToLower() -replace '\[\]$','') -replace '^system\.',''
    if($wType -notcontains $tn){$bad="refused: type [$($n.TypeName.Name)] is not allowed";break}
  }
  elseif($t -eq 'VariableExpressionAst'){
    $vp=$n.VariablePath
    if(@('executioncontext','host','pshome','profile') -contains $vp.UserPath.ToLower() -or ($vp.IsDriveQualified -and $vp.DriveName -ne 'env')){$bad="refused: variable `$$($vp.UserPath) is not allowed";break}
  }
  elseif($t -eq 'AssignmentStatementAst'){
    $l=$n.Left
    $plain=($l -is [Management.Automation.Language.VariableExpressionAst] -and -not $l.VariablePath.IsDriveQualified) -or ($l -is [Management.Automation.Language.IndexExpressionAst] -and $l.Target -is [Management.Automation.Language.VariableExpressionAst] -and -not $l.Target.VariablePath.IsDriveQualified)
    if(-not $plain){$bad='refused: you may only assign to plain variables';break}
  }
}
if($bad){J $bad;exit}
$ErrorActionPreference='Continue'
$ExecutionContext.SessionState.LanguageMode='ConstrainedLanguage'
$r=@(Invoke-Expression $src)
if($r.Count -eq 0){'[]'}
elseif(@($r|?{$_ -is [string]}).Count -eq $r.Count){$r -join "`n"}
elseif($r[0].PSObject.TypeNames[0] -like 'Microsoft.PowerShell.Commands.Internal.Format*'){($r|Out-String -Width 200).Trim()}
else{ConvertTo-Json -InputObject $r -Compress -Depth 3}
}catch{J $_.Exception.Message}"##;

/// Runs a model-written script through PS_GUARD. 20 s, then the whole process tree is stopped.
fn ps_script(src: &str) -> Result<String, String> {
    let mut c = super::util::hidden(&system32(r"WindowsPowerShell\v1.0\powershell.exe").to_string_lossy());
    c.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &b64_utf16(PS_GUARD)]).env("LP_SCRIPT", src);
    let (out, err) = super::util::run_for(c, 20)?;
    Ok(if out.trim().is_empty() { d::error(err.trim()) } else { out })
}

fn b64_utf16(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }); }
    }
    out
}

fn system32(exe: &str) -> PathBuf { PathBuf::from(std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32").join(exe) }

/// Runs one built-in script with its checked values as environment variables.
fn ps(body: &str, a: &Value, env: &[(&str, String)]) -> Result<String, String> {
    let mut c = super::util::hidden(&system32(r"WindowsPowerShell\v1.0\powershell.exe").to_string_lossy());
    c.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &b64_utf16(&format!("{PRE}{body}{POST}"))]).env("LP_N", d::limit(a)?.to_string());
    for (k, v) in env { c.env(k, v); }
    let (out, err) = super::util::run_for(c, SECS)?;
    Ok(if out.trim().is_empty() { d::error(err.trim()) } else { out })
}

/// A fixed program with fixed arguments; stdout, or stderr when there is none.
fn exe(c: std::process::Command) -> Result<String, String> {
    let (out, err) = super::util::run_for(c, SECS)?;
    Ok(if out.trim().is_empty() { err } else { out })
}

fn run(tool: &str, what: &str, a: &Value) -> Result<String, String> {
    let n = d::limit(a)?;
    match (tool, what) {
        ("disk", "folder_size") => folder_size(&safe_path(d::text_arg(a, "path"), &roots())?, n),
        ("disk", "large_files") => large_files(&safe_path(d::text_arg(a, "path"), &roots())?, n),
        ("disk", "temp_sizes") => temp_sizes(),
        ("sys", "power") => {
            let mut c = super::util::hidden(&system32("powercfg.exe").to_string_lossy());
            c.arg("/getactivescheme");
            Ok(json!({"plan": exe(c)?.trim()}).to_string())
        }
        ("ps", _) => ps_script(&d::script(a)?),
        ("net", "wifi") => wifi(),
        ("net", "hosts") => {
            let t = std::fs::read_to_string(system32(r"drivers\etc\hosts")).map_err(|e| e.to_string())?;
            Ok(json!(t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).take(n as usize).collect::<Vec<_>>()).to_string())
        }
        ("file", w) => file(w, &safe_path(d::text_arg(a, "path"), &roots())?, n),
        ("dev", "versions") => Ok(versions()),
        ("dev", w) => git(w, &safe_path(d::text_arg(a, "path"), &roots())?, n),
        ("reg", _) => match d::text_arg(a, "key") {
            "uninstall" => ps(INSTALLED, a, &[("LP_A", String::new())]),
            k => ps(REG_READ, a, &[("LP_K", REG.iter().find(|r| r.0 == k).ok_or(format!("key must be one of: {}", d::REG_KEYS.join(", ")))?.1.to_string())]),
        },
        _ => {
            let body = script(tool, what).ok_or("not available")?;
            let env = match (tool, what) {
                ("proc", "details" | "services") | ("apps", _) => vec![("LP_A", d::name(a)?)],
                ("proc", "port_owner") => vec![("LP_P", d::port(a)?.to_string())],
                ("net", "ping" | "dns") => vec![("LP_A", d::host(a)?)],
                ("net", "connect") => vec![("LP_A", d::host(a)?), ("LP_P", d::port(a)?.to_string())],
                _ => vec![],
            };
            ps(body, a, &env)
        }
    }
}

/// Runs one tool call and returns what the model sees.
fn call(tool: &str, a: &Value) -> String {
    let t0 = Instant::now();
    let what = d::text_arg(a, "what");
    let out = match d::cat(tool) {
        Some(c) if c.whats.contains(&what) => run(tool, what, a),
        Some(c) => Err(format!("what must be one of: {}", c.whats.join(", "))),
        None => Err("unknown tool".into()),
    };
    let cap = if tool == "events" || tool == "ps" || (tool, what) == ("file", "read") { BIG_CAP } else { CAP };
    let text = d::shape(&out.unwrap_or_else(|e| d::error(if e == "timeout" { "timed out after 10 s" } else { &e })), cap);
    audit(tool, a, text.len(), t0.elapsed());
    text
}

// ---- Files and folders ----

/// Documents, Downloads and Desktop (wherever Windows keeps them), resolved.
fn roots() -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Shell::{SHGetKnownFolderPath, FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads};
    [FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Desktop].iter().filter_map(|id| unsafe {
        let mut p = std::ptr::null_mut();
        let ok = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) == 0 && !p.is_null();
        let s = ok.then(|| String::from_utf16_lossy(std::slice::from_raw_parts(p, (0..).take_while(|&i| *p.add(i) != 0).count())));
        windows_sys::Win32::System::Com::CoTaskMemFree(p as _);
        std::fs::canonicalize(s?).ok()
    }).collect()
}

/// An absolute local path inside `roots` once links and junctions are resolved, and not a private location.
fn safe_path(p: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if p.is_empty() || p.len() > 260 || p.chars().any(|c| c.is_control() || "<>|\"?*".contains(c)) { return Err("give one absolute path".into()); }
    if p.starts_with(r"\\") || p.starts_with("//") { return Err("network and device paths are not allowed".into()); }
    let path = Path::new(p);
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) { return Err("give an absolute path without ..".into()); }
    let real = std::fs::canonicalize(path).map_err(|_| "not found".to_string())?;
    if !roots.iter().any(|r| real.starts_with(r)) { return Err("outside the allowed folders (Documents, Downloads, Desktop)".into()); }
    if private(&real) { return Err("that location looks private (keys, passwords, browser or app data)".into()); }
    Ok(real)
}

/// Keys, credentials, browser and password-manager data, hive files, LitePilot's own data.
fn private(p: &Path) -> bool {
    const DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".claude", ".codex", "credentials", "vault", "protect", "user data", "profiles",
        "keepass", "keepassxc", "1password", "bitwarden", "lastpass", "dashlane", "keeper", "litepilot", "litepilotdata"];
    const FILES: &[&str] = &["sam", "system", "security", "auth.json", "cookies", "login data", "web data", ".git-credentials", ".npmrc", ".pypirc", ".netrc", "_netrc"];
    let parts: Vec<String> = p.components().filter_map(|c| match c { Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()), _ => None }).collect();
    let last = parts.last().map_or("", |s| s.as_str());
    parts.iter().any(|c| DIRS.contains(&c.as_str()) || c.starts_with(".env"))
        || FILES.contains(&last) || last.starts_with("ntuser")
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"].iter().any(|k| last.starts_with(k))
        || [".pfx", ".p12", ".key", ".pem", ".kdbx", ".ppk", ".jks", ".keystore"].iter().any(|e| last.ends_with(e))
}

fn shown(p: &Path) -> String { p.to_string_lossy().trim_start_matches(r"\\?\").to_string() }
fn mb(b: u64) -> f64 { (b as f64 / 1_048_576.0 * 10.0).round() / 10.0 }

/// Visits every file under `root` (links and junctions are not followed) until `end`. False if time ran out.
fn walk(root: &Path, end: Instant, f: &mut dyn FnMut(&Path, u64)) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if Instant::now() > end { return false; }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() { stack.push(e.path()); } else if t.is_file() { f(&e.path(), e.metadata().map_or(0, |m| m.len())); }
        }
    }
    true
}

fn soon() -> Instant { Instant::now() + Duration::from_secs(SECS - 2) }

fn folder_size(root: &Path, n: u64) -> Result<String, String> {
    let (mut total, mut files, mut by) = (0u64, 0u64, std::collections::HashMap::<String, u64>::new());
    let done = walk(root, soon(), &mut |p, len| {
        total += len;
        files += 1;
        let top = p.strip_prefix(root).ok().and_then(|r| r.components().next()).map_or(String::new(), |c| c.as_os_str().to_string_lossy().into_owned());
        *by.entry(top).or_default() += len;
    });
    let mut top: Vec<_> = by.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    let top: Vec<Value> = top.iter().take(n as usize).map(|(k, v)| json!({"name": k, "mb": mb(*v)})).collect();
    Ok(json!({"path": shown(root), "mb": mb(total), "files": files, "partial": (!done).then_some(true), "biggest": top}).to_string())
}

fn large_files(root: &Path, n: u64) -> Result<String, String> {
    let mut big: Vec<(u64, PathBuf)> = Vec::new();
    let n = n as usize;
    let done = walk(root, soon(), &mut |p, len| {
        if private(p) { return; }
        big.push((len, p.to_path_buf()));
        if big.len() > n * 4 { big.sort_by_key(|b| std::cmp::Reverse(b.0)); big.truncate(n); }
    });
    big.sort_by_key(|b| std::cmp::Reverse(b.0));
    let rows: Vec<Value> = big.iter().take(n).map(|(l, p)| json!({"file": shown(p), "mb": mb(*l)})).collect();
    Ok(json!({"partial": (!done).then_some(true), "files": rows}).to_string())
}

fn temp_sizes() -> Result<String, String> {
    let env = |k: &str| PathBuf::from(std::env::var_os(k).unwrap_or_default());
    let dirs = [env("TEMP"), env("SystemRoot").join("Temp"), env("LOCALAPPDATA").join("CrashDumps"),
        env("SystemRoot").join(r"SoftwareDistribution\Download"), env("LOCALAPPDATA").join(r"Microsoft\Windows\INetCache")];
    let end = soon();
    let mut o = serde_json::Map::new();
    for d in dirs.iter().filter(|d| d.is_dir()) {
        let mut total = 0;
        let done = walk(d, end, &mut |_, len| total += len);
        o.insert(shown(d), json!(if done { json!(mb(total)) } else { json!(format!("{}+ (partial)", mb(total))) }));
    }
    Ok(Value::Object(o).to_string())
}

fn file(what: &str, p: &Path, n: u64) -> Result<String, String> {
    const TEXT: &[&str] = &["txt", "log", "md", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml", "ini", "cfg", "conf", "toml", "rs", "py", "js", "mjs", "ts", "tsx", "jsx",
        "html", "htm", "css", "scss", "c", "h", "cpp", "hpp", "cs", "java", "kt", "go", "rb", "php", "ps1", "psm1", "bat", "cmd", "sh", "sql", "lua", "swift", "gitignore",
        "properties", "gradle", "srt", "vtt", "diff", "patch"];
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    match what {
        "read" => {
            use std::io::Read;
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase());
            if !meta.is_file() || !ext.as_deref().is_none_or(|e| TEXT.contains(&e)) { return Err("only text files can be read".into()); }
            let mut buf = Vec::new();
            std::fs::File::open(p).and_then(|f| f.take(MAX_READ).read_to_end(&mut buf)).map_err(|e| e.to_string())?;
            if buf.contains(&0) { return Err("that looks like a binary file".into()); }
            Ok(String::from_utf8_lossy(&buf).trim_start_matches('\u{feff}').to_string())
        }
        "list" => {
            let mut rows: Vec<String> = std::fs::read_dir(p).map_err(|e| e.to_string())?.flatten().filter(|e| !private(&e.path())).map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                match e.metadata() { Ok(m) if m.is_dir() => format!("{name}\\"), Ok(m) => format!("{name} {}KB", m.len().div_ceil(1024)), _ => name }
            }).collect();
            rows.sort_by_key(|r| r.to_lowercase());
            let more = rows.len().saturating_sub(n as usize);
            rows.truncate(n as usize);
            Ok(json!({"entries": rows, "more": (more > 0).then_some(more)}).to_string())
        }
        _ => Ok(json!({"path": shown(p), "dir": meta.is_dir(), "bytes": meta.is_file().then_some(meta.len()), "modified": meta.modified().ok().map(iso),
            "created": meta.created().ok().map(iso), "readonly": meta.permissions().readonly()}).to_string()),
    }
}

// ---- Fixed programs ----

fn wifi() -> Result<String, String> {
    let mut c = super::util::hidden(&system32("netsh.exe").to_string_lossy());
    c.args(["wlan", "show", "interfaces"]);
    let t = exe(c)?;
    let o: serde_json::Map<String, Value> = t.lines().filter_map(|l| l.split_once(" : ")).map(|(k, v)| (k.trim().to_string(), json!(v.trim()))).collect();
    Ok(if o.is_empty() { json!({"wifi": t.trim()}) } else { Value::Object(o) }.to_string())
}

fn versions() -> String {
    let mut o = serde_json::Map::new();
    for (name, arg) in [("git", "--version"), ("node", "--version"), ("python", "--version"), ("cargo", "--version"), ("rustc", "--version"), ("dotnet", "--version"), ("go", "version"), ("java", "-version")] {
        let mut c = super::util::hidden(name);
        c.arg(arg);
        let v = match super::util::run_for(c, 5) {
            Ok((out, err)) => out.lines().chain(err.lines()).map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(80).collect(),
            Err(e) if e == "timeout" => "timed out".into(),
            Err(_) => "not found".to_string(),
        };
        o.insert(name.into(), json!(v));
    }
    Value::Object(o).to_string()
}

/// Read-only git: no pager, colour, fsmonitor hook, external diff or text conversion, and no index lock.
fn git(what: &str, repo: &Path, n: u64) -> Result<String, String> {
    let mut c = super::util::hidden("git");
    c.arg("-C").arg(repo).args(["--no-pager", "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "-c", "color.ui=false"])
        .env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0");
    let n = n.to_string();
    match what {
        "git_status" => c.args(["status", "--porcelain=v1", "-b"]),
        "git_log" => c.args(["log", "-n", n.as_str(), "--date=short", "--format=%h %ad %an %s"]),
        _ => c.args(["diff", "--no-ext-diff", "--no-textconv", "--stat", "-p", "-U2"]),
    };
    exe(c).map_err(|e| if e.starts_with("could not start") { "git was not found on PATH".into() } else { e })
}

// ---- Bookkeeping ----

/// UTC time as 2026-11-08T12:00:00Z.
fn iso(t: SystemTime) -> String {
    let s = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, r) = (s.div_euclid(86400), s.rem_euclid(86400));
    // Days to civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", r / 3600, r % 3600 / 60, r % 60)
}

/// One line per call in diag-audit.log: when, tool, parameters (redacted), result size, time taken. Never the output.
fn audit(tool: &str, a: &Value, bytes: usize, took: Duration) {
    if cfg!(test) { return; }
    use std::io::Write;
    let f = super::util::data_dir().join("diag-audit.log");
    if f.metadata().is_ok_and(|m| m.len() > 256 * 1024) { let _ = std::fs::rename(&f, f.with_extension("log.old")); }
    let line = format!("{} {tool} {} {bytes}B {}ms\n", iso(SystemTime::now()), d::redact(&a.to_string()), took.as_millis());
    if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(f) { let _ = h.write_all(line.as_bytes()); }
}

/// Counts calls per message (the app bumps TURN_FILE before each one); true once the message is over its share.
fn over_limit(turn: &mut (String, u32)) -> bool {
    let now = std::fs::read_to_string(super::util::data_dir().join(TURN_FILE)).unwrap_or_default();
    if now != turn.0 { *turn = (now, 0); }
    turn.1 += 1;
    turn.1 > MAX_CALLS
}

fn handle(line: &str, tools: &[&'static str], turn: &mut (String, u32)) -> Option<String> {
    let req: Value = serde_json::from_str(line).ok()?;
    let id = req.get("id")?.clone();
    let reply = match req["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({"protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": SERVER, "version": "1"}})),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": d::schemas(tools)})),
        "tools/call" => {
            let (name, args) = (req["params"]["name"].as_str().unwrap_or(""), &req["params"]["arguments"]);
            let text = if !tools.contains(&name) { d::shape(&d::error("unknown tool"), CAP) }
                else if over_limit(turn) { d::shape(&d::error("tool limit for this message reached; answer with what you have"), CAP) }
                else { call(name, args) };
            Ok(json!({"content": [{"type": "text", "text": text}]}))
        }
        m => Err(json!({"code": -32601, "message": format!("method not found: {m}")})),
    };
    Some(match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    }.to_string())
}

/// Runs as the tool server for the listed categories: requests on stdin, one at a time, answers on stdout.
pub fn serve(cats: &str) {
    use std::io::{BufRead, Write};
    let tools: Vec<&'static str> = cats.split(',').filter_map(|c| d::cat(c.trim()).map(|c| c.name)).collect();
    let (stdin, mut out, mut turn) = (std::io::stdin(), std::io::stdout(), (String::new(), 0));
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(reply) = handle(&line, &tools, &mut turn) {
            if writeln!(out, "{reply}").is_err() || out.flush().is_err() { break; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lp-diag-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    #[test]
    fn paths_stay_inside_the_allowed_folders() {
        let root = tmp("paths");
        let roots = [root.clone()];
        for f in ["a.txt", "id_rsa", "x.pem", ".env.local", "NTUSER.DAT"] { std::fs::write(root.join(f), "x").unwrap(); }
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh").join("config"), "x").unwrap();
        let s = |p: PathBuf| shown(&p);
        assert!(safe_path(&s(root.join("a.txt")), &roots).is_ok());
        for bad in ["id_rsa", "x.pem", ".env.local", "NTUSER.DAT", ".ssh", r".ssh\config"] { assert!(safe_path(&s(root.join(bad)), &roots).is_err(), "{bad}"); }
        let dots = format!(r"{}\..\{}\a.txt", shown(&root), root.file_name().unwrap().to_string_lossy());
        assert!(safe_path(&dots, &roots).is_err(), "{dots}: {:?}", safe_path(&dots, &roots));
        for bad in [r"\\server\share\a.txt", r"\\?\C:\Windows\win.ini", r"\\.\PhysicalDrive0", "relative\\a.txt", r"C:\Windows\win.ini", "", "C:\\x\na.txt"] {
            assert!(safe_path(bad, &roots).is_err(), "{bad:?}");
        }
        // A junction inside an allowed folder that leads outside it is refused once resolved.
        let j = root.join("out");
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&j).arg(r"C:\Windows").output().unwrap();
        assert!(made.status.success(), "mklink failed");
        assert!(safe_path(&s(j.join("win.ini")), &roots).is_err());
        let _ = std::fs::remove_dir(&j);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_reads_are_text_only_and_capped() {
        let root = tmp("files");
        std::fs::write(root.join("big.log"), "secret_token=abc123\n".repeat(10_000)).unwrap();
        std::fs::write(root.join("a.exe"), "MZ").unwrap();
        std::fs::write(root.join("bin.txt"), [0u8, 1, 2]).unwrap();
        let out = d::shape(&file("read", &root.join("big.log"), 20).unwrap(), BIG_CAP);
        assert!(out.len() <= BIG_CAP + 13 && out.ends_with("[truncated]</data>") && !out.contains("abc123"));
        assert!(file("read", &root.join("a.exe"), 20).is_err() && file("read", &root.join("bin.txt"), 20).is_err());
        assert!(file("list", &root, 20).unwrap().contains("big.log"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timeout_kills_the_whole_tree() {
        // PowerShell starts a child that would write a file after 4 s; stopping at 1 s must take the child too.
        let flag = std::env::temp_dir().join(format!("lp-diag-tree-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&flag);
        let mut c = super::super::util::hidden("powershell.exe");
        c.args(["-NoProfile", "-Command", &format!("Start-Process cmd -ArgumentList '/c ping -n 5 127.0.0.1 >nul & echo x > \"{}\"' -WindowStyle Hidden; Start-Sleep 30", flag.display())]);
        let t0 = Instant::now();
        assert_eq!(super::super::util::run_for(c, 1).unwrap_err(), "timeout");
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_secs(6));
        assert!(!flag.exists(), "the grandchild outlived the timeout");
    }

    #[test]
    fn injection_in_any_value_is_refused() {
        for bad in ["; calc", "`calc`", "$(calc)", "a\"b", "a'b", "a\nb"] {
            for (tool, what, key) in [("net", "ping", "host"), ("net", "dns", "host"), ("net", "connect", "host"), ("proc", "details", "name"), ("proc", "services", "name"),
                ("apps", "installed", "name"), ("apps", "env_vars", "name"), ("file", "read", "path"), ("file", "list", "path"), ("disk", "folder_size", "path"),
                ("disk", "large_files", "path"), ("dev", "git_status", "path"), ("reg", "read", "key")] {
                let out = call(tool, &json!({"what": what, key: bad, "port": 80}));
                assert!(out.contains("\"error\""), "{tool}.{what} {key}={bad:?}: {out}");
            }
            assert!(call("sys", &json!({"what": bad})).contains("\"error\""));
            assert!(call("nope", &json!({"what": "os"})).contains("unknown tool"));
        }
        assert!(call("net", &json!({"what": "connect", "host": "127.0.0.1", "port": 0})).contains("port must be"));
    }

    #[test]
    fn ps_runs_read_only_scripts_and_refuses_the_rest() {
        let run = |s: &str| call("ps", &json!({"what": "run", "script": s}));
        for good in ["Get-Process | Sort-Object WS -Descending | Select-Object -First 2 Name,Id | ConvertTo-Json -Compress",
            "gps | ? { $_.WS -gt 1MB } | select -f 1 | % { $_.Name }", "(Get-CimInstance Win32_OperatingSystem).Caption", "'{0:N1}' -f (2.567 * 1)",
            "$h=@{}; $h['a']=1; foreach($k in $h.Keys){ \"$k=$($h[$k])\" }", "Get-ChildItem $env:SystemRoot -EA 0 | select -f 3 Name | ft | Out-String", "hostname"] {
            let o = run(good);
            assert!(!o.contains("refused") && !o.contains("\"error\""), "{good}: {o}");
        }
        assert!(run("'{0:N1}' -f 2.567").contains("2.6"));
        let marker = std::env::temp_dir().join(format!("lp-ps-write-{}.txt", std::process::id()));
        let m = marker.display().to_string();
        for bad in [format!("'x' > '{m}'"), format!("Set-Content '{m}' x"), format!("Out-File '{m}'"), "Remove-Item C:\\nope".into(), "Get-Content C:\\Windows\\win.ini".into(),
            "& calc".into(), ". calc".into(), "calc".into(), "cmd /c dir".into(), "iex 'gps'".into(), "Invoke-Expression 'gps'".into(), "Start-Process calc".into(),
            "(Get-Process notepad).Kill()".into(), "[IO.File]::Delete('x')".into(), "[Diagnostics.Process]::Start('calc')".into(), "$env:X='1'".into(),
            "(Get-Process)[0].PriorityClass='Idle'".into(), "function f{1}".into(), "$ExecutionContext.SessionState.LanguageMode".into(), "Get-Process; Stop-Process -Id 1".into(),
            "gps | % { & $_ }".into(), "Get-Process | Export-Csv x.csv".into(), "Set-Alias gps Remove-Item".into(), "ipconfig /release".into(), "powershell -c 1".into(),
            "[scriptblock]::Create('1')".into(), "New-Object Net.WebClient".into(), "Invoke-WebRequest http://x".into(), "$x=1; $x.GetType()".into(), "Select-String a C:\\x".into()] {
            let o = run(&bad);
            assert!(o.contains("\"error\"") || o.contains("refused"), "{bad}: {o}");
        }
        assert!(!marker.exists());
        assert!(run("").contains("1-2000") && run(&"1;".repeat(1500)).contains("1-2000"));
    }

    #[test]
    fn server_lists_only_its_tools() {
        let mut turn = (String::new(), 0);
        let r = handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#, &["lang"], &mut turn).unwrap();
        assert!(r.contains("languages_keyboards") && !r.contains("\"sys\""));
        let r = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sys","arguments":{"what":"os"}}}"#, &["lang"], &mut turn).unwrap();
        assert!(r.contains("unknown tool"));
    }

    #[test]
    fn dates_and_base64() {
        assert_eq!(iso(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(iso(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400 + 3661)), "2000-02-29T01:01:01Z");
        assert_eq!(b64_utf16("a"), "YQA=");
    }

    #[test]
    fn keyboards_are_listed() {
        let out = call("lang", &json!({"what": "languages_keyboards"}));
        assert!(out.contains("\"tag\"") && out.contains("\"kb\""), "{out}");
    }

    /// Runs every `what` on this machine and prints what came back (cargo test smoke -- --ignored --nocapture).
    #[test]
    #[ignore = "runs every diagnostic query on this PC"]
    fn smoke() {
        // A folder and a text file inside Downloads: this repository, when it is checked out there.
        let repo = std::env::current_dir().unwrap();
        for c in d::CATS {
            for w in c.whats {
                let path = if (c.name, *w) == ("file", "read") { repo.join("README.md") } else { repo.clone() };
                let a = json!({"what": w, "path": shown(&path), "host": "example.com", "port": 443, "name": "host", "key": "winver", "limit": 5});
                let t0 = Instant::now();
                let out = call(c.name, &a);
                let status = if out.contains("requires_elevation") { "ELEVATION" } else if out.contains("{\"error\"") { "FAIL" } else { "DATA" };
                println!("{status:9} {:>5}ms {:>5}B {}.{w}  {}", t0.elapsed().as_millis(), out.len(), c.name, out.chars().take(160).collect::<String>());
            }
        }
    }
}
