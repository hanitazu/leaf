# Installiert Leaf für den aktuellen Benutzer und registriert ihn als Browser.
# Keine Admin-Rechte nötig. Deinstallieren: .\install.ps1 -Uninstall
param([switch]$Uninstall, [switch]$Update)
$ErrorActionPreference = 'Stop'
$dir  = Join-Path $env:LOCALAPPDATA 'Programs\Leaf'
$exe  = Join-Path $dir 'leaf.exe'
$lnk  = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Leaf.lnk'
$cli  = 'HKCU:\Software\Clients\StartMenuInternet\Leaf'
$cls  = 'HKCU:\Software\Classes\LeafHTML'

if ($Uninstall) {
    Get-Process leaf -ErrorAction SilentlyContinue | Stop-Process -Force
    Remove-Item $cli, $cls -Recurse -Force -ErrorAction SilentlyContinue
    Remove-ItemProperty 'HKCU:\Software\RegisteredApplications' -Name Leaf -ErrorAction SilentlyContinue
    Remove-Item $lnk, $dir -Recurse -Force -ErrorAction SilentlyContinue
    'Leaf entfernt.'; return
}

Get-Process leaf -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'leaf.exe') $exe -Force

function Set-Key($path, $values) {
    New-Item -Path $path -Force | Out-Null
    foreach ($k in $values.Keys) {
        if ($k -eq '') { Set-Item -Path $path -Value $values[$k] }
        else { New-ItemProperty -Path $path -Name $k -Value $values[$k] -PropertyType String -Force | Out-Null }
    }
}

# Dokument-/URL-Typ
Set-Key $cls @{ '' = 'Leaf HTML Document'; 'URL Protocol' = '' }
Set-Key "$cls\DefaultIcon" @{ '' = "`"$exe`",0" }
Set-Key "$cls\shell\open\command" @{ '' = "`"$exe`" `"%1`"" }

# Browser-Registrierung
Set-Key $cli @{ '' = 'Leaf' }
Set-Key "$cli\DefaultIcon" @{ '' = "`"$exe`",0" }
Set-Key "$cli\shell\open\command" @{ '' = "`"$exe`"" }
Set-Key "$cli\Capabilities" @{
    'ApplicationName' = 'Leaf'
    'ApplicationDescription' = 'Schlanker, schneller Browser'
    'ApplicationIcon' = "`"$exe`",0"
}
Set-Key "$cli\Capabilities\StartMenu" @{ 'StartMenuInternet' = 'Leaf' }
Set-Key "$cli\Capabilities\URLAssociations" @{ 'http' = 'LeafHTML'; 'https' = 'LeafHTML' }
Set-Key "$cli\Capabilities\FileAssociations" @{ '.htm' = 'LeafHTML'; '.html' = 'LeafHTML'; '.pdf' = 'LeafHTML'; '.svg' = 'LeafHTML' }
New-Item 'HKCU:\Software\RegisteredApplications' -Force -ErrorAction SilentlyContinue | Out-Null
New-ItemProperty 'HKCU:\Software\RegisteredApplications' -Name Leaf -Value 'Software\Clients\StartMenuInternet\Leaf\Capabilities' -PropertyType String -Force | Out-Null

# Startmenü-Verknüpfung
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$s.TargetPath = $exe; $s.IconLocation = "$exe,0"; $s.Description = 'Leaf Browser'; $s.Save()

"Installiert nach $exe"
# Windows erlaubt Programmen nicht, sich selbst als Standard zu setzen –
# diese Seite öffnet direkt Leafs Eintrag, dort 'Als Standard festlegen' klicken.
if (-not $Update) { Start-Process 'ms-settings:defaultapps?registeredAppUser=Leaf' }
