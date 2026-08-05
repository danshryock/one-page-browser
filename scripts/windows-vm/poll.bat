@echo off
REM Runs inside the Windows VM's real interactive desktop session (started
REM by bootstrap.sh, either manually the first time or automatically via
REM the Startup-folder shortcut on later boots). Polls the shared folder
REM (\\host.lan\Data, see lib.sh on the Linux side) once a second for
REM request-<id>.flag files; when one shows up, runs the matching
REM cmd-<id>.bat and reports back via output-<id>.txt/exitcode-<id>.txt/
REM done-<id>.flag.
REM
REM Every request uses a fresh <id>, never a reused filename — the VM's SMB
REM client caches file content per path and doesn't reliably notice the host
REM overwriting a file directly on disk (bypassing SMB), so a fixed
REM cmd.bat/output.txt pair got stale results back. See lib.sh's vm_run.
REM
REM Deliberately just a batch file, not a scheduled task running as SYSTEM —
REM a SYSTEM-context task can build things fine but can't usefully launch or
REM interact with a GUI app (Windows' Session 0 isolation), which this VM
REM is also used for (screenshot-based smoke tests, see screenshot.sh).
if not exist C:\ClaudeBrowser mkdir C:\ClaudeBrowser
:loop
REM `dir /b` + `2>nul`, not a plain wildcard `for %%f in (...)`, so an idle
REM tick (the common case) doesn't print "Could Not Find ...*.flag" to this
REM console on every single poll.
for /f "delims=" %%f in ('dir /b \\host.lan\Data\request-*.flag 2^>nul') do call :handle "%%~nf"
timeout /t 1 /nobreak > nul
goto loop

:handle
set "ID=%~1"
set "ID=%ID:request-=%"
call \\host.lan\Data\cmd-%ID%.bat > \\host.lan\Data\output-%ID%.txt 2>&1
echo %ERRORLEVEL% > \\host.lan\Data\exitcode-%ID%.txt
del \\host.lan\Data\request-%ID%.flag
echo done > \\host.lan\Data\done-%ID%.flag
goto :eof
