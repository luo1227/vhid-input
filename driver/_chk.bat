@echo off
call "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvars64.bat" >/dev/null 2>&1
echo vcvars-error=%errorlevel%
cl /nologo /c /D_AMD64_ /D_WIN64 /DAMD64 /D_UNICODE /DUNICODE /DNTDDI_VERSION=0x0A00000C /D_WIN32_WINNT=0x0A00 /DKMDF_VERSION_MAJOR=1 /DKMDF_VERSION_MINOR=31 /D_KERNEL_MODE=1 /I"C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\km" /I"C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\km\crt" /I"C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\shared" /I"C:\Program Files (x86)\Windows Kits\10\Include\wdf\kmdf\1.31" _chk.c
echo cl-error=%errorlevel%
