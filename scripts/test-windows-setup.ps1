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

    private static IntPtr FindInput(int process) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, data) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != process || !IsWindowVisible(window)) return true;
            if (GetDlgItem(window, 1001) != IntPtr.Zero) {
                found = window;
                return false;
            }
            // The desktop entry asks whether to configure before opening its first input.
            if (GetDlgItem(window, 6) != IntPtr.Zero && GetDlgItem(window, 7) != IntPtr.Zero)
                PostMessage(window, 0x0111, (UIntPtr)6, IntPtr.Zero);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static void CompleteStep(Process process, string prompt, string value, bool password) {
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("Setup exited before showing its input window");
            var window = FindInput(process.Id);
            var actualPrompt = window == IntPtr.Zero ? null : ReadText(GetDlgItem(window, 1000));
            if (actualPrompt != null && actualPrompt.Contains(prompt)) {
                var input = GetDlgItem(window, 1001);
                if (((GetWindowLong(input, -16) & 0x20) != 0) != password)
                    throw new Exception("Unexpected setup password masking");
                if (password && ReadText(input) != "")
                    throw new Exception("Setup password input was not initially empty");
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

foreach ($binaryName in @('viberwhisper.exe', 'viberwhisper-app.exe')) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = (Resolve-Path (Join-Path $BinaryDirectory $binaryName)).Path
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    if ($binaryName -eq 'viberwhisper.exe') { $startInfo.ArgumentList.Add('setup') }
    $setupProcess = [Diagnostics.Process]::Start($startInfo)
    try {
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
