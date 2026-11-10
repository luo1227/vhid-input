@echo off
rem Stand-alone build: no MSBuild, no WDK props (the kit's VS integration is
rem missing on this machine). Sets up the WDK include/lib path by hand and
rem drives cl.exe directly.
setlocal
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
if errorlevel 1 goto :fail

set WDKROOT=C:\Program Files (x86)\Windows Kits\10
set WDKINC=%WDKROOT%\Include\10.0.26100.0
set WDKLIB=%WDKROOT%\Lib\10.0.26100.0
set WDFINC=%WDKROOT%\Include\wdf\kmdf\1.31
set WDFLIB=%WDKROOT%\Lib\wdf\kmdf\x64\1.31
set OUT=%~dp0..\bin
if not exist "%OUT%" mkdir "%OUT%"

cd /d "%~dp0"

rem cl writes the .obj files into obj\, which it will not create itself; a
rem clean checkout (obj/ is gitignored) failed to build without this line.
if not exist obj mkdir obj

cl /nologo /c /W4 /O2 /GS /kernel /Z7 /D_AMD64_ /D_WIN64 /DAMD64 /D_UNICODE /DUNICODE /DNTDDI_VERSION=0x0A00000C /D_WIN32_WINNT=0x0A00 /DKMDF_VERSION_MAJOR=1 /DKMDF_VERSION_MINOR=31 /Gs /I"%WDKINC%\km" /I"%WDKINC%\km\crt" /I"%WDKINC%\shared" /I"%WDFINC%" /Fo:obj\ vhidinput.c vhidinput_report.c
if errorlevel 1 goto :fail

rem /DEBUG keeps a private PDB next to the .sys so kd can map offsets
rem (vhidinput+0x1b1a) back to source lines when a dump is analysed.
link /nologo /DRIVER /SUBSYSTEM:NATIVE /OUT:"%OUT%\vhidinput.sys" /ENTRY:FxDriverEntry /LIBPATH:"%WDKLIB%\km\x64" /LIBPATH:"%WDFLIB%" /DEF:vhidinput.def /DEBUG /LTCG /OPT:REF,ICF obj\vhidinput.obj obj\vhidinput_report.obj wdfldr.lib wdfdriverentry.lib vhfkm.lib BufferOverflowFastFailK.lib ntoskrnl.lib hal.lib
if errorlevel 1 goto :fail

echo [mk] built %OUT%\vhidinput.sys
endlocal & exit /b 0

:fail
echo [mk] BUILD FAILED
endlocal & exit /b 1
