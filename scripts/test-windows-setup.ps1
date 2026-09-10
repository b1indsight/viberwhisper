# Run on an isolated Windows account. Exercise the real release executables without saving config.
param([Parameter(Mandatory)][string]$BinaryDirectory)
$ErrorActionPreference = "Stop"
$configPath = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'ViberWhisper/config.json'
if (Test-Path $configPath) { throw "Setup smoke test requires an account without ViberWhisper configuration" }

Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class SetupDialogSmoke {
    private delegate bool EnumProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr window, uint command);
    [DllImport("user32.dll")] private static extern IntPtr GetDlgItem(IntPtr window, int id);
    [DllImport("user32.dll", EntryPoint="GetWindowLongW")] private static extern int GetWindowLong(IntPtr window, int index);
    [DllImport("user32.dll", EntryPoint="PostMessageW")] private static extern bool PostMessage(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW", CharSet=CharSet.Unicode)]
    private static extern IntPtr ReadMessage(IntPtr window, uint message, UIntPtr wparam, StringBuilder text, uint flags, uint timeout, out UIntPtr result);
    [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW", CharSet=CharSet.Unicode)]
    private static extern IntPtr WriteMessage(IntPtr window, uint message, UIntPtr wparam, string text, uint flags, uint timeout, out UIntPtr result);

    private static string ReadText(IntPtr window) {
        var text = new StringBuilder(4096);
        UIntPtr result;
        // A previous step may be destroyed between enumeration and this bounded read.
        if (ReadMessage(window, 0x000D, (UIntPtr)text.Capacity, text, 2, 2000, out result) == IntPtr.Zero)
            return null;
        return text.ToString();
    }

    private static IntPtr FindDialog(int process, int control) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, data) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != process || !IsWindowVisible(window)) return true;
            if (GetDlgItem(window, control) != IntPtr.Zero) {
                found = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static void ConfirmFirstRun(Process process) {
        // Check the initial confirmation separately: an input-only test can miss its fallback.
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("App exited before first-run confirmation");
            var window = FindDialog(process.Id, 6);
            if (window != IntPtr.Zero && GetDlgItem(window, 7) != IntPtr.Zero) {
                var message = ReadText(GetDlgItem(window, 0xffff));
                if (message == null || !message.Contains("尚未找到配置文件"))
                    throw new Exception("Unexpected first-run confirmation");
                if (GetWindow(window, 4) != IntPtr.Zero) // GW_OWNER
                    throw new Exception("First-run confirmation borrowed an external owner");
                if (!PostMessage(window, 0x0111, (UIntPtr)6, IntPtr.Zero))
                    throw new Exception("First-run confirmation did not accept Yes");
                return;
            }
            Thread.Sleep(25);
        }
        throw new Exception("First-run confirmation did not appear within 30 seconds");
    }

    public static void CompleteStep(Process process, string prompt, string value, bool password) {
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("Setup exited before showing its input window");
            var window = FindDialog(process.Id, 1001);
            var actualPrompt = window == IntPtr.Zero ? null : ReadText(GetDlgItem(window, 1000));
            if (actualPrompt != null && actualPrompt.Contains(prompt)) {
                var input = GetDlgItem(window, 1001);
                if (((GetWindowLong(input, -16) & 0x20) != 0) != password)
                    throw new Exception("Unexpected setup password masking");
                // Password text cannot be inspected reliably from this other process. Empty
                // defaults are checked in-process by ui/setup/windows/tests.rs in this same job.
                if (!password) {
                    UIntPtr result;
                    if (WriteMessage(input, 0x000C, UIntPtr.Zero, value, 2, 2000, out result) == IntPtr.Zero || result == UIntPtr.Zero)
                        throw new Exception("Setup control did not accept input");
                }
                // Cancel at the password step, before credentials, verification, or saving.
                if (!PostMessage(window, 0x0111, (UIntPtr)(password ? 2 : 1), IntPtr.Zero))
                    throw new Exception("Setup dialog did not accept its completion command");
                return;
            }
            Thread.Sleep(25);
        }
        throw new Exception("Setup input did not appear within 30 seconds");
    }
}
'@

$launches = @(
    @{ Name = 'viberwhisper.exe'; Setup = $true; Shell = $false; Ssh = $false },
    @{ Name = 'viberwhisper.exe'; Setup = $false; Shell = $false; Ssh = $false },
    @{ Name = 'viberwhisper-app.exe'; Setup = $false; Shell = $false; Ssh = $false },
    @{ Name = 'viberwhisper-app.exe'; Setup = $false; Shell = $true; Ssh = $false },
    @{ Name = 'viberwhisper-app.exe'; Setup = $false; Shell = $false; Ssh = $true },
    @{ Name = 'viberwhisper-app.exe'; Setup = $false; Shell = $true; Ssh = $true }
)
foreach ($launch in $launches) {
    $binaryName = $launch.Name
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = (Resolve-Path (Join-Path $BinaryDirectory $binaryName)).Path
    $startInfo.UseShellExecute = $launch.Shell
    $startInfo.CreateNoWindow = $true
    if ($launch.Setup) { $startInfo.ArgumentList.Add('setup') }
    # ShellExecute inherits the caller's environment. Restore it immediately after spawning.
    $previousSsh = [Environment]::GetEnvironmentVariable('SSH_CLIENT', 'Process')
    $previousDisplay = [Environment]::GetEnvironmentVariable('DISPLAY', 'Process')
    try {
        [Environment]::SetEnvironmentVariable('SSH_CLIENT', $(if ($launch.Ssh) { '127.0.0.1 12345 22' } else { $null }), 'Process')
        [Environment]::SetEnvironmentVariable('DISPLAY', $null, 'Process')
        $setupProcess = [Diagnostics.Process]::Start($startInfo)
    } finally {
        [Environment]::SetEnvironmentVariable('SSH_CLIENT', $previousSsh, 'Process')
        [Environment]::SetEnvironmentVariable('DISPLAY', $previousDisplay, 'Process')
    }
    try {
        if (!$launch.Setup) {
            [SetupDialogSmoke]::ConfirmFirstRun($setupProcess)
            Write-Host "${binaryName}: first-run confirmation appeared (shell=$($launch.Shell), SSH_CLIENT=$($launch.Ssh))."
        }
        [SetupDialogSmoke]::CompleteStep($setupProcess, 'STT API 地址', 'https://example.com/v1/audio/transcriptions', $false)
        [SetupDialogSmoke]::CompleteStep($setupProcess, 'STT 模型名称', 'whisper-test', $false)
        [SetupDialogSmoke]::CompleteStep($setupProcess, 'API', '', $true)
        if (!$setupProcess.WaitForExit(10000)) { throw "$binaryName did not exit after setup cancellation" }
        if ($setupProcess.ExitCode -ne 0) { throw "$binaryName returned exit code $($setupProcess.ExitCode)" }
        if (Test-Path $configPath) { throw "$binaryName saved configuration after cancellation" }
        Write-Host "${binaryName}: text and masked-password windows appeared; cancellation exited without saving."
    } finally {
        if (!$setupProcess.HasExited) {
            $setupProcess.Kill($true)
            $setupProcess.WaitForExit()
        }
        $setupProcess.Dispose()
    }
}
