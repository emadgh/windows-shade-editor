param(
    [switch]$NoEditorCopy
)

$ErrorActionPreference = 'Stop'

function Test-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-Administrator)) {
    $args = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    if ($NoEditorCopy) { $args += '-NoEditorCopy' }
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $args | Out-Null
    exit
}

$sourceDir = Split-Path -Parent $PSCommandPath
$dllSource = Join-Path $sourceDir 'ShadeEditorShell.dll'
$schemaSource = Join-Path $sourceDir 'ShadeEditor.propdesc'
$editorSource = Join-Path (Split-Path -Parent $sourceDir) 'ShadeEditor.exe'
if (-not (Test-Path $editorSource)) {
    $editorSource = Join-Path $sourceDir 'ShadeEditor.exe'
}
if (-not (Test-Path $dllSource)) { throw "Missing $dllSource" }
if (-not (Test-Path $schemaSource)) { throw "Missing $schemaSource" }

$version = '0.12.0'
$clsid = '{6F49F9D5-0F3A-4BF0-8C74-8A59951A75D2}'
$thumbHandler = '{E357FCCD-A995-4576-B01F-234630154E96}'
$progId = 'ShadeEditor.Project'

$shellDir = Join-Path $env:ProgramFiles "Shade Editor\Shell\$version"
New-Item -ItemType Directory -Path $shellDir -Force | Out-Null
$dllTarget = Join-Path $shellDir 'ShadeEditorShell.dll'
$schemaTarget = Join-Path $shellDir 'ShadeEditor.propdesc'
Copy-Item $dllSource $dllTarget -Force
Copy-Item $schemaSource $schemaTarget -Force

$editorTarget = $null
if (-not $NoEditorCopy -and (Test-Path $editorSource)) {
    $editorDir = Join-Path $env:LOCALAPPDATA 'Programs\ShadeEditor'
    New-Item -ItemType Directory -Path $editorDir -Force | Out-Null
    $editorTarget = Join-Path $editorDir 'ShadeEditor.exe'
    Copy-Item $editorSource $editorTarget -Force
}

$interop = @'
using System;
using System.Runtime.InteropServices;
public static class ShadeShellNative {
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)]
    public static extern int PSRegisterPropertySchema(string path);
    [DllImport("propsys.dll", CharSet = CharSet.Unicode)]
    public static extern int PSUnregisterPropertySchema(string path);
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
}
'@
Add-Type -TypeDefinition $interop -ErrorAction SilentlyContinue
[void][ShadeShellNative]::PSUnregisterPropertySchema($schemaTarget)
$hr = [ShadeShellNative]::PSRegisterPropertySchema($schemaTarget)
if ($hr -lt 0) { throw ('PSRegisterPropertySchema failed: 0x{0:X8}' -f [uint32]$hr) }

# COM class and the property-handler binding must be machine-wide. Microsoft
# documents PropertySystem\PropertyHandlers under HKLM.
$clsidKey = "Registry::HKEY_LOCAL_MACHINE\Software\Classes\CLSID\$clsid"
New-Item -Path $clsidKey -Force | Out-Null
Set-Item -Path $clsidKey -Value 'Shade Editor Shell Handler'
$inproc = Join-Path $clsidKey 'InProcServer32'
New-Item -Path $inproc -Force | Out-Null
Set-Item -Path $inproc -Value $dllTarget
New-ItemProperty -Path $inproc -Name 'ThreadingModel' -Value 'Apartment' -PropertyType String -Force | Out-Null

$approved = 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved'
New-Item -Path $approved -Force | Out-Null
New-ItemProperty -Path $approved -Name $clsid -Value 'Shade Editor Shell Handler' -PropertyType String -Force | Out-Null

$propertyHandlers = 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\PropertySystem\PropertyHandlers\.shade'
New-Item -Path $propertyHandlers -Force | Out-Null
Set-Item -Path $propertyHandlers -Value $clsid

# File association and presentation are per-user, so this does not replace a
# machine-wide policy for other workstation users.
$classes = 'Registry::HKEY_CURRENT_USER\Software\Classes'
$extension = Join-Path $classes '.shade'
New-Item -Path $extension -Force | Out-Null
Set-Item -Path $extension -Value $progId
New-ItemProperty -Path $extension -Name 'Content Type' -Value 'application/x-shade-editor-project' -PropertyType String -Force | Out-Null

$prog = Join-Path $classes $progId
New-Item -Path $prog -Force | Out-Null
Set-Item -Path $prog -Value 'Shade Editor Project'
New-ItemProperty -Path $prog -Name 'FriendlyTypeName' -Value 'Shade Editor Project' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $prog -Name 'Treatment' -Value 2 -PropertyType DWord -Force | Out-Null

$thumbKey = Join-Path $prog "ShellEx\$thumbHandler"
New-Item -Path $thumbKey -Force | Out-Null
Set-Item -Path $thumbKey -Value $clsid

$fullDetails = 'prop:System.PropGroup.FileSystem;System.ItemNameDisplay;System.ItemTypeText;System.Size;System.DateModified;System.PropGroup.Image;ShadeEditor.PhysicalDimensions;ShadeEditor.PixelDimensions;ShadeEditor.Dpi;ShadeEditor.BitDepth;ShadeEditor.ColorModel;ShadeEditor.ChannelCount;ShadeEditor.BaseChannelCount;ShadeEditor.FaceCount;ShadeEditor.ActiveFace;ShadeEditor.SourceFileName;ShadeEditor.SavedAt'
$previewDetails = 'prop:ShadeEditor.PhysicalDimensions;ShadeEditor.PixelDimensions;ShadeEditor.Dpi;ShadeEditor.BitDepth;ShadeEditor.ColorModel;ShadeEditor.ChannelCount;ShadeEditor.FaceCount;ShadeEditor.ActiveFace'
$infoTip = 'prop:System.ItemTypeText;ShadeEditor.PhysicalDimensions;ShadeEditor.ColorModel;ShadeEditor.ChannelCount;ShadeEditor.FaceCount'
New-ItemProperty -Path $prog -Name 'FullDetails' -Value $fullDetails -PropertyType String -Force | Out-Null
New-ItemProperty -Path $prog -Name 'PreviewDetails' -Value $previewDetails -PropertyType String -Force | Out-Null
New-ItemProperty -Path $prog -Name 'InfoTip' -Value $infoTip -PropertyType String -Force | Out-Null

if ($editorTarget) {
    $command = Join-Path $prog 'shell\open\command'
    New-Item -Path $command -Force | Out-Null
    Set-Item -Path $command -Value ('"{0}" "%1"' -f $editorTarget)
    $icon = Join-Path $prog 'DefaultIcon'
    New-Item -Path $icon -Force | Out-Null
    Set-Item -Path $icon -Value ('"{0}",0' -f $editorTarget)
}

# SHCNE_ASSOCCHANGED. Explorer may still use an already-cached thumbnail until
# the folder is refreshed or Explorer is restarted.
[ShadeShellNative]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host 'Shade Editor Shell integration installed.' -ForegroundColor Green
Write-Host "Shell DLL: $dllTarget"
Write-Host "Property schema: $schemaTarget"
if ($editorTarget) { Write-Host "Editor: $editorTarget" }
Write-Host 'Refresh Explorer (F5). If an old cached thumbnail remains, restart Explorer once.'
