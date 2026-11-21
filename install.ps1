# Installiert Leaf für den aktuellen Benutzer und registriert ihn als Browser.
# Keine Admin-Rechte nötig. Deinstallieren: .\install.ps1 -Uninstall
# -AllUsers (als Administrator): Leaf zusätzlich systemweit registrieren –
# manche Windows-Versionen zeigen Leaf sonst nicht unter "Standard-Apps" an.
param([switch]$Uninstall, [switch]$Update, [switch]$AllUsers)
$ErrorActionPreference = 'Stop'
$dir  = Join-Path $env:LOCALAPPDATA 'Programs\Leaf'
$exe  = Join-Path $dir 'leaf.exe'
$lnk  = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Leaf.lnk'
$hive = if ($AllUsers) { 'HKLM:' } else { 'HKCU:' }
$cli  = "$hive\Software\Clients\StartMenuInternet\Leaf"
$cls  = "$hive\Software\Classes\LeafHTML"

if ($Uninstall) {
    Get-Process leaf -ErrorAction SilentlyContinue | Stop-Process -Force
    Remove-Item $cli, $cls -Recurse -Force -ErrorAction SilentlyContinue
    Remove-ItemProperty "$hive\Software\RegisteredApplications" -Name Leaf -ErrorAction SilentlyContinue
    if (-not $AllUsers) { Remove-Item $lnk, $dir -Recurse -Force -ErrorAction SilentlyContinue }
    'Leaf entfernt.'; return
}

$src = Join-Path $PSScriptRoot 'leaf.exe'
if (Test-Path $src) {
    Get-Process leaf -ErrorAction SilentlyContinue | Stop-Process -Force
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item $src $exe -Force
} elseif (-not (Test-Path $exe)) {
    throw "leaf.exe nicht gefunden (weder neben dem Skript noch in $dir)"
}

function Set-Key($path, $values) {
    New-Item -Path $path -Force | Out-Null
    foreach ($k in $values.Keys) {
        if ($k -eq '') { Set-Item -Path $path -Value $values[$k] }
        else { New-ItemProperty -Path $path -Name $k -Value $values[$k] -PropertyType String -Force | Out-Null }
    }
}

$icon = "$exe,0"
$desc = 'Schlanker, schneller Browser'

# Dokument-/URL-Typ
Set-Key $cls @{ '' = 'Leaf HTML Document'; 'URL Protocol' = '' }
Set-Key "$cls\Application" @{ 'ApplicationName' = 'Leaf'; 'ApplicationIcon' = $icon; 'ApplicationDescription' = $desc; 'ApplicationCompany' = 'Leaf' }
Set-Key "$cls\DefaultIcon" @{ '' = $icon }
Set-Key "$cls\shell\open\command" @{ '' = "`"$exe`" `"%1`"" }

# Browser-Registrierung
Set-Key $cli @{ '' = 'Leaf' }
Set-Key "$cli\DefaultIcon" @{ '' = $icon }
Set-Key "$cli\shell\open\command" @{ '' = "`"$exe`"" }
Set-Key "$cli\Capabilities" @{ 'ApplicationName' = 'Leaf'; 'ApplicationDescription' = $desc; 'ApplicationIcon' = $icon }
Set-Key "$cli\Capabilities\StartMenu" @{ 'StartMenuInternet' = 'Leaf' }
Set-Key "$cli\Capabilities\URLAssociations" @{ 'http' = 'LeafHTML'; 'https' = 'LeafHTML' }
Set-Key "$cli\Capabilities\FileAssociations" @{ '.htm' = 'LeafHTML'; '.html' = 'LeafHTML'; '.pdf' = 'LeafHTML'; '.svg' = 'LeafHTML' }
New-Item "$hive\Software\RegisteredApplications" -Force -ErrorAction SilentlyContinue | Out-Null
New-ItemProperty "$hive\Software\RegisteredApplications" -Name Leaf -Value 'Software\Clients\StartMenuInternet\Leaf\Capabilities' -PropertyType String -Force | Out-Null

# Startmenü-Verknüpfung
if (-not $AllUsers) {
    $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
    $s.TargetPath = $exe; $s.IconLocation = $icon; $s.Description = 'Leaf Browser'; $s.Save()
}

# Windows über die neuen Zuordnungen informieren
Add-Type -Namespace Leaf -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int e, uint f, IntPtr a, IntPtr b);'
[Leaf.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

"Installiert nach $exe"
# Windows erlaubt Programmen nicht, sich selbst als Standard zu setzen –
# diese Seite öffnet direkt Leafs Eintrag, dort 'Als Standard festlegen' klicken.
if (-not $Update) { Start-Process 'ms-settings:defaultapps?registeredAppUser=Leaf' }
