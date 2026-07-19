@echo off

setlocal

set "SRC=%~dp0java"

set "OUT=%~dp0prebuilt"

set "CLASSES=%OUT%\classes"

set "JAR=%OUT%\rsift-bootstrap.jar"



if not exist "%CLASSES%" mkdir "%CLASSES%"



where javac >nul 2>nul

if %ERRORLEVEL% neq 0 (

    echo [WARN] javac not found — cannot build rsift-bootstrap.jar

    exit /b 1

)



echo [INFO] Building bootstrap jar (JDK-only + PlatformBridge + ChunkBridge + Hooks + RenderHooks)...

javac -encoding UTF-8 -d "%CLASSES%" ^
    "%SRC%\com\rsift\RsiftAgentState.java" ^
    "%SRC%\com\rsift\RsiftBootstrapAgent.java" ^
    "%SRC%\com\rsift\RsiftClassTransformer.java" ^
    "%SRC%\com\rsift\RsiftModBridge.java" ^
    "%SRC%\com\rsift\RsiftRuntimeHooks.java" ^
    "%SRC%\com\rsift\RsiftRenderHooks.java" ^
    "%SRC%\com\rsift\RsiftUiBridge.java" ^
    "%SRC%\com\rsift\RsiftScreenHooks.java" ^
    "%SRC%\com\rsift\RsiftPressHandler.java" ^
    "%SRC%\com\rsift\RsiftHooks.java" ^
    "%SRC%\com\rsift\RsiftPlatformBridge.java" ^
    "%SRC%\com\rsift\RsiftChunkBridge.java"

if %ERRORLEVEL% neq 0 exit /b 1



if not exist "%OUT%" mkdir "%OUT%"

echo Premain-Class: com.rsift.RsiftBootstrapAgent> "%OUT%\MANIFEST.MF"

echo Can-Redefine-Classes: true>> "%OUT%\MANIFEST.MF"

echo Can-Retransform-Classes: true>> "%OUT%\MANIFEST.MF"



cd /d "%CLASSES%"

jar cfm "%JAR%" "%OUT%\MANIFEST.MF" com\rsift\*.class

if %ERRORLEVEL% neq 0 exit /b 1



echo [INFO] Built %JAR%

exit /b 0

