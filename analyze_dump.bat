@echo off
set KD=C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\kd.exe
set OUT=C:\Users\admin\AppData\Local\Temp\vhid_kd.log
rem The driver PDB is not installed next to the .sys; point kd at a staged copy
rem so crash stacks map to source lines instead of vhidinput+0x1234.
set PDBSRC=C:\Users\admin\AppData\Local\Temp\vhidpdbs
del "%OUT%" 2>nul
"%KD%" -z %1 -logo "%OUT%" -y "%PDBSRC%" -c ".symfix+ %PDBSRC%;.reload;!analyze -v;!dpcwatchdog;!stacks 2;q" >nul 2>&1
type "%OUT%"
