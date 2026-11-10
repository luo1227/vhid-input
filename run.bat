@echo off
rem Build the driver, then rebuild and run the staged self-test.
rem
rem Stage 1 only moves the cursor to the centre once. Run it first: if the
rem machine survives that, the submit path is sound and the rest of the
rem stages are only about ordering and timing.
setlocal
call "%~dp0driver\mk.bat"
if errorlevel 1 goto :fail

pushd "%~dp0user"
cargo build --example selftest
if errorlevel 1 goto :failpop

target\debug\examples\selftest.exe %*
set RC=%ERRORLEVEL%
popd
endlocal & exit /b %RC%

:failpop
popd
:fail
echo [run] FAILED
endlocal & exit /b 1
