$ErrorActionPreference = 'Stop'

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Administrator)) {
    Start-Process powershell.exe -Verb RunAs -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"") | Out-Null
    exit
}

$clsid = '{6F49F9D5-0F3A-4BF0-8C74-8A59951A75D2}'
$progId = 'ShadeEditor.Project'
$shellRoot = Join-Path $env:ProgramFiles 'Shade Editor\Shell'

$native = @'
using System;
using System.Runtime.InteropServices;
public static class ShadeShellUninstallNative {
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)]
    public static extern int PSUnregisterPropertySchema(string path);
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
}
'@
Add-Type -TypeDefinition $native -ErrorAction SilentlyContinue

if (Test-Path $shellRoot) {
    Get-ChildItem $shellRoot -Filter ShadeEditor.propdesc -Recurse -ErrorAction SilentlyContinue | ForEach-Object {
        [void][ShadeShellUninstallNative]::PSUnregisterPropertySchema($_.FullName)
    }
}

$handler = 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers\.shade'
if (Test-Path $handler) {
    $value = (Get-Item $handler).GetValue('')
    if ($value -eq $clsid) { Remove-Item $handler -Recurse -Force }
}

$approved = 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved'
if (Test-Path $approved) {
    Remove-ItemProperty $approved -Name $clsid -Force -ErrorAction SilentlyContinue
}

$classKey = "Registry::HKEY_LOCAL_MACHINE\Software\Classes\CLSID\$clsid"
if (Test-Path $classKey) { Remove-Item $classKey -Recurse -Force }

$userClasses = 'Registry::HKEY_CURRENT_USER\Software\Classes'
$extension = Join-Path $userClasses '.shade'
if ((Test-Path $extension) -and ((Get-Item $extension).GetValue('') -eq $progId)) {
    Remove-Item $extension -Recurse -Force
}
$prog = Join-Path $userClasses $progId
if (Test-Path $prog) { Remove-Item $prog -Recurse -Force }

[ShadeShellUninstallNative]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

try {
    if (Test-Path $shellRoot) { Remove-Item $shellRoot -Recurse -Force }
} catch {
    Write-Warning 'Explorer is still using the Shell DLL. Registration is removed; delete the Shell folder after Explorer or Windows restarts.'
}

Write-Host 'Shade Editor Shell integration removed.' -ForegroundColor Green
