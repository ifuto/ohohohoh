# Build a valid javaagent JAR (forward-slash paths, META-INF/MANIFEST.MF first).
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$classes = Join-Path $root "prebuilt/classes"
$jar = Join-Path $root "prebuilt/rsift-bootstrap.jar"
$src = Join-Path $root "java/com/rsift"
$libDir = Join-Path $root "lib"
$asmJar = Join-Path $libDir "asm-9.7.1.jar"

if (-not (Test-Path $classes)) {
    New-Item -ItemType Directory -Force -Path $classes | Out-Null
}
if (-not (Test-Path $libDir)) {
    New-Item -ItemType Directory -Force -Path $libDir | Out-Null
}
if (-not (Test-Path $asmJar)) {
    Write-Host "[INFO] Downloading ASM 9.7.1 for Screen.init hook..."
    Invoke-WebRequest -Uri "https://repo1.maven.org/maven2/org/ow2/asm/asm/9.7.1/asm-9.7.1.jar" -OutFile $asmJar
}

$javac = (Get-Command javac -ErrorAction SilentlyContinue)
if (-not $javac) {
    Write-Error "javac not found — install JDK to rebuild rsift-bootstrap.jar"
}

& $javac.Source -encoding UTF-8 -d $classes `
    (Join-Path $src "RsiftAgentState.java") `
    (Join-Path $src "RsiftPressHandler.java") `
    (Join-Path $src "RsiftUiBridge.java") `
    (Join-Path $src "RsiftModBridge.java") `
    (Join-Path $src "RsiftRuntimeHooks.java") `
    (Join-Path $src "RsiftScreenHooks.java") `
    (Join-Path $src "RsiftRenderHooks.java") `
    (Join-Path $src "RsiftHooks.java") `
    (Join-Path $src "RsiftPlatformBridge.java") `
    (Join-Path $src "RsiftClassTransformer.java") `
    (Join-Path $src "RsiftBootstrapAgent.java")

$nettyJar = Get-ChildItem -Path (Join-Path $env:APPDATA ".minecraft\libraries") -Recurse -Filter "netty-transport-*.jar" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($nettyJar) {
    Write-Host "[INFO] Compiling RsiftPacketTap with $($nettyJar.FullName)"
    & $javac.Source -encoding UTF-8 -cp "$($nettyJar.FullName);$classes" -d $classes (Join-Path $src "RsiftPacketTap.java")
} else {
    Write-Warning "Netty jar not found — RsiftPacketTap skipped (packet tap disabled until libraries cached; use build.bat for full Netty tap)"
}

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

if (Test-Path $jar) { Remove-Item $jar -Force }

$fs = [System.IO.File]::Open($jar, [System.IO.FileMode]::CreateNew)
$zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)

function Add-ZipTextEntry($zip, [string]$name, [string]$text) {
    $e = $zip.CreateEntry($name, [System.IO.Compression.CompressionLevel]::Optimal)
    $w = New-Object System.IO.StreamWriter($e.Open(), [System.Text.UTF8Encoding]::new($false))
    $w.Write($text)
    $w.Close()
}

function Add-ZipFileEntry($zip, [string]$name, [string]$path) {
    $e = $zip.CreateEntry($name, [System.IO.Compression.CompressionLevel]::Optimal)
    $in = [System.IO.File]::OpenRead($path)
    $out = $e.Open()
    $in.CopyTo($out)
    $in.Close()
    $out.Close()
}

$manifest = "Manifest-Version: 1.0`r`nPremain-Class: com.rsift.RsiftBootstrapAgent`r`nCan-Redefine-Classes: true`r`nCan-Retransform-Classes: true`r`n`r`n"
Add-ZipTextEntry $zip "META-INF/MANIFEST.MF" $manifest
foreach ($cls in @(
    "RsiftBootstrapAgent", "RsiftClassTransformer", "RsiftAgentState", "RsiftPressHandler", "RsiftUiBridge",
    "RsiftModBridge", "RsiftRuntimeHooks", "RsiftScreenHooks", "RsiftRenderHooks",
    "RsiftHooks", "RsiftPlatformBridge"
)) {
    Add-ZipFileEntry $zip "com/rsift/$cls.class" (Join-Path $classes "com/rsift/$cls.class")
}
$tapClass = Join-Path $classes "com/rsift/RsiftPacketTap.class"
if (Test-Path $tapClass) {
    Add-ZipFileEntry $zip "com/rsift/RsiftPacketTap.class" $tapClass
}
Write-Host "[OK] Included PlatformBridge + Hooks + Screen.init hook + RsiftUiBridge + RsiftModBridge"

$zip.Dispose()
$fs.Close()

Write-Host "[OK] Built $jar ($((Get-Item $jar).Length) bytes)"

$java = (Get-Command java).Source
$dll = (Resolve-Path "C:\Users\sungs\Downloads\rsift\target\release\rsift_jvm.dll" -ErrorAction SilentlyContinue)
if ($dll) {
    $agentArgs = "nativeDll=$dll"
    & $java "-javaagent:${jar}=$agentArgs" -version *> $null
    if ($LASTEXITCODE -ne 0) { exit 1 }
} else {
    & $java "-javaagent:${jar}=nativeDll=C:\Users\sungs\Downloads\rsift\target\release\rsift_jvm.dll" -version *> $null
    if ($LASTEXITCODE -ne 0) { exit 1 }
}
