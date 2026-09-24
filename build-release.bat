@echo off
setlocal enabledelayedexpansion

rem Build the release binary and copy it to .\releases.
rem The output filename is read from Cargo.toml: prefers [[bin]] name,
rem falls back to [package] name — matching Cargo's own output rules.

set "SCRIPT_DIR=%~dp0"
if "%SCRIPT_DIR:~-1%"=="\" set "SCRIPT_DIR=%SCRIPT_DIR:~0,-1%"

set "CARGO_TOML=%SCRIPT_DIR%\Cargo.toml"
set "RELEASE_DIR=%SCRIPT_DIR%\releases"
set "TARGET_DIR=%SCRIPT_DIR%\target\release"

if not exist "%CARGO_TOML%" (
    echo [error] Cargo.toml not found at "%CARGO_TOML%"
    pause
    exit /b 1
)

rem Resolve cargo (PATH first, then default rustup install).
set "CARGO=cargo"
where cargo >nul 2>nul
if errorlevel 1 (
    if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
        set "CARGO=%USERPROFILE%\.cargo\bin\cargo.exe"
    ) else (
        echo [error] cargo not found on PATH and %%USERPROFILE%%\.cargo\bin\cargo.exe missing.
        pause
        exit /b 1
    )
)

rem Extract the binary name from Cargo.toml via a tiny PowerShell helper.
set "PS_HELPER=%TEMP%\cargo-bin-name-%RANDOM%.ps1"
> "%PS_HELPER%" echo $c = Get-Content -Raw -LiteralPath $args[0]
>> "%PS_HELPER%" echo if ($c -match '(?ms)\[\[bin\]\][^\[]*?^\s*name\s*=\s*"([^"]+)"') { Write-Output $Matches[1]; exit 0 }
>> "%PS_HELPER%" echo if ($c -match '(?ms)\[package\][^\[]*?^\s*name\s*=\s*"([^"]+)"') { Write-Output $Matches[1]; exit 0 }
>> "%PS_HELPER%" echo exit 1

set "BIN_NAME="
for /f "usebackq tokens=* delims=" %%i in (`powershell -NoProfile -ExecutionPolicy Bypass -File "%PS_HELPER%" "%CARGO_TOML%"`) do set "BIN_NAME=%%i"
del /q "%PS_HELPER%" >nul 2>nul

if "%BIN_NAME%"=="" (
    echo [error] failed to read binary name from Cargo.toml
    pause
    exit /b 1
)

set "TARGET_EXE=%TARGET_DIR%\%BIN_NAME%.exe"
set "OUTPUT_EXE=%RELEASE_DIR%\%BIN_NAME%.exe"

echo [name]  binary = %BIN_NAME%
echo [build] "%CARGO%" build --release
pushd "%SCRIPT_DIR%" >nul
"%CARGO%" build --release
set "BUILD_ERR=%errorlevel%"
popd >nul
if not "%BUILD_ERR%"=="0" (
    echo [error] cargo build failed with exit code %BUILD_ERR%.
    pause
    exit /b %BUILD_ERR%
)

if not exist "%TARGET_EXE%" (
    echo [error] expected output missing: "%TARGET_EXE%"
    pause
    exit /b 1
)

if not exist "%RELEASE_DIR%" (
    mkdir "%RELEASE_DIR%" || (
        echo [error] failed to create "%RELEASE_DIR%"
        pause
        exit /b 1
    )
)

echo [copy]  "%TARGET_EXE%" -^> "%OUTPUT_EXE%"
copy /y "%TARGET_EXE%" "%OUTPUT_EXE%" >nul
if errorlevel 1 (
    echo [error] copy failed.
    pause
    exit /b 1
)

echo [done]  release binary available at "%OUTPUT_EXE%"
endlocal
pause
exit /b 0
