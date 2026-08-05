# Recovery tool, not part of the normal pipeline: if a launch attempt hits a
# missing-dependency error (the kind build-and-test.sh's sidecar-file
# deployment and the VM's Windows App SDK Runtime install are meant to
# prevent), the resulting dialog can wedge poll.bat — its `call` blocks on
# the crashed/stuck exe, so the poller stops answering anything until the
# dialog is gone. Ordinary interaction doesn't reach it reliably: it may be
# a modal owned by a *different*, protected process (`csrss.exe`, for the
# classic "X.dll was not found" hard-error box — confirmed via `tasklist /V`
# once; `taskkill` refuses it as a critical process), so mouse clicks and
# keyboard focus can't be counted on. WM_CLOSE via SendMessage, targeted
# straight at the window handle, doesn't need focus or a real click — run
# this manually (`powershell -ExecutionPolicy Bypass -File
# C:\ClaudeBrowser\close_dialog.ps1`, via lib.sh's vm_run) if build-and-test.sh
# times out waiting for a response.
Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);' -Name U32 -Namespace W
$procs = Get-Process | Where-Object MainWindowTitle
foreach ($p in $procs) {
    if ($p.MainWindowTitle -like "*System Error*" -or $p.MainWindowTitle -like "*could not be started*" -or $p.MainWindowTitle -like "*browser-windows-reactor.exe*" -or $p.MainWindowTitle -like "*browser_windows_reactor*") {
        [W.U32]::SendMessage($p.MainWindowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        Write-Output "sent WM_CLOSE to PID $($p.Id): $($p.MainWindowTitle)"
    }
}
