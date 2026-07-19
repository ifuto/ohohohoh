@echo off
setlocal
set "SRC=%~dp0java"
set "OUT=%~dp0..\target\bootstrap"
set "JAR=%OUT%\rsift-bootstrap.jar"

if not exist "%OUT%" mkdir "%OUT%"

where javac >nul 2>nul
if %ERRORLEVEL% neq 0 (
    echo [WARN] javac not found — bootstrap jar must be built manually
    exit /b 0
)

set "CP="
for /r "%APPDATA%\.minecraft\libraries" %%F in (client-1.21.11*.jar) do set "CP=%%F"
if not defined CP (
    for /r "%APPDATA%\.minecraft\versions\1.21.11" %%F in (*.jar) do set "CP=%%F"
)

if not defined CP (
    echo [WARN] Minecraft 1.21.11 jar not found for bootstrap compile — using agent-only mode
    exit /b 0
)

echo [INFO] Compiling Rsift bootstrap agent...
javac -encoding UTF-8 -cp "%CP%" -d "%OUT%" "%SRC%\com\rsift\*.java"
if %ERRORLEVEL% neq 0 exit /b 1

cd /d "%OUT%"
jar cfe "%JAR%" com.rsift.RsiftBootstrapAgent com\rsift\*.class
echo [INFO] Built %JAR%
