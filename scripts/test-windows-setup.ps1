# Run on an isolated Windows account. Exercise the real release executables without saving config.
param(
    [Parameter(Mandatory)][string]$BinaryDirectory,
    [string]$ScreenshotDirectory
)
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
    public struct WindowRectangle { public int Left, Top, Right, Bottom; }
    public static Action<IntPtr, string> CaptureWindow;
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out WindowRectangle rectangle);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr context, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    private delegate bool EnumProc(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] private static extern IntPtr GetWindow(IntPtr window, uint command);
    [DllImport("user32.dll")] private static extern IntPtr GetDlgItem(IntPtr window, int id);
    [DllImport("user32.dll")] private static extern IntPtr GetWindowDpiAwarenessContext(IntPtr window);
    [DllImport("user32.dll")] private static extern bool AreDpiAwarenessContextsEqual(IntPtr first, IntPtr second);
    [DllImport("user32.dll")] private static extern uint GetDpiForWindow(IntPtr window);
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

    private static void CheckDpi(IntPtr window) {
        if (!AreDpiAwarenessContextsEqual(GetWindowDpiAwarenessContext(window), new IntPtr(-4)))
            throw new Exception("Setup window is not Per-Monitor V2 aware before winit starts");
        var dpi = GetDpiForWindow(window);
        if (dpi == 0) throw new Exception("Setup window has no valid DPI");
        Console.WriteLine("Setup HWND: PMv2, actual DPI=" + dpi);
    }

    public static void ConfirmFirstRun(Process process) {
        // Check the initial confirmation separately: an input-only test can miss its fallback.
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("App exited before first-run confirmation");
            var window = FindDialog(process.Id, 6);
            if (window != IntPtr.Zero && GetDlgItem(window, 7) != IntPtr.Zero) {
                var message = ReadText(GetDlgItem(window, 1000));
                if (message == null || !message.Contains("尚未找到配置文件"))
                    throw new Exception("Unexpected first-run confirmation");
                CheckDpi(window);
                if (GetWindow(window, 4) != IntPtr.Zero) // GW_OWNER
                    throw new Exception("First-run confirmation borrowed an external owner");
                CaptureWindow?.Invoke(window, "first-run confirmation");
                if (!PostMessage(window, 0x0111, (UIntPtr)6, GetDlgItem(window, 6)))
                    throw new Exception("First-run confirmation did not accept Yes");
                return;
            }
            Thread.Sleep(25);
        }
        throw new Exception("First-run confirmation did not appear within 30 seconds");
    }

    public static void CompleteStep(Process process, string prompt, string value, bool password, bool cancel) {
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("Setup exited before showing its input window");
            var window = FindDialog(process.Id, 1001);
            var actualPrompt = window == IntPtr.Zero ? null : ReadText(GetDlgItem(window, 1000));
            if (actualPrompt != null && actualPrompt.Contains(prompt)) {
                var input = GetDlgItem(window, 1001);
                CheckDpi(window);
                if (GetDpiForWindow(input) != GetDpiForWindow(window))
                    throw new Exception("Setup input and dialog use different DPI");
                if (((GetWindowLong(input, -16) & 0x20) != 0) != password)
                    throw new Exception("Unexpected setup password masking");
                // Password text cannot be inspected reliably from this other process. Empty
                // defaults are checked in-process by ui/setup/windows/tests.rs in this same job.
                if (!password && !cancel) {
                    UIntPtr result;
                    if (WriteMessage(input, 0x000C, UIntPtr.Zero, value, 2, 2000, out result) == IntPtr.Zero || result == UIntPtr.Zero)
                        throw new Exception("Setup control did not accept input");
                }
                // This isolated account has no personal configuration; never capture passwords.
                if (!password) CaptureWindow?.Invoke(window, prompt);
                // Cancel before credentials, verification, or saving in either recognition mode.
                if (!PostMessage(window, 0x0111, (UIntPtr)(cancel ? 2 : 1), IntPtr.Zero))
                    throw new Exception("Setup dialog did not accept its completion command");
                return;
            }
            Thread.Sleep(25);
        }
        throw new Exception("Setup input did not appear within 30 seconds");
    }

    public static void DismissMessage(Process process, string prompt) {
        var deadline = Stopwatch.StartNew();
        while (deadline.Elapsed < TimeSpan.FromSeconds(30)) {
            if (process.HasExited) throw new Exception("App exited before showing its message");
            var window = FindDialog(process.Id, 1000);
            if (window != IntPtr.Zero && GetDlgItem(window, 1001) == IntPtr.Zero) {
                var message = ReadText(GetDlgItem(window, 1000));
                if (message != null && message.Contains(prompt)) {
                    CheckDpi(window);
                    CaptureWindow?.Invoke(window, "required-input message");
                    if (!PostMessage(window, 0x0010, UIntPtr.Zero, IntPtr.Zero))
                        throw new Exception("Informational message did not accept Close");
                    return;
                }
            }
            Thread.Sleep(25);
        }
        throw new Exception("Setup message did not appear within 30 seconds");
    }
}
'@

if ($ScreenshotDirectory) {
    Add-Type -AssemblyName System.Drawing
    New-Item -ItemType Directory -Force -Path $ScreenshotDirectory | Out-Null
    $script:screenshotNumber = 0
    [SetupDialogSmoke]::CaptureWindow = {
        param([IntPtr]$Window, [string]$Label)
        # Read physical bounds even when the PowerShell host uses a different DPI context.
        $previousDpi = [SetupDialogSmoke]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))
        if ($previousDpi -eq [IntPtr]::Zero) { throw "Cannot set screenshot thread to PMv2" }
        try {
            $bounds = [SetupDialogSmoke+WindowRectangle]::new()
            if (![SetupDialogSmoke]::GetWindowRect($Window, [ref]$bounds)) { throw "Cannot measure setup window" }
            $bitmap = [Drawing.Bitmap]::new($bounds.Right - $bounds.Left, $bounds.Bottom - $bounds.Top, [Drawing.Imaging.PixelFormat]::Format24bppRgb)
            try {
                $graphics = [Drawing.Graphics]::FromImage($bitmap)
                try {
                    $context = $graphics.GetHdc()
                    try {
                        if (![SetupDialogSmoke]::PrintWindow($Window, $context, 0)) { throw "Cannot render setup window" }
                    } finally { $graphics.ReleaseHdc($context) }
                } finally { $graphics.Dispose() }
                $script:screenshotNumber++
                $path = Join-Path $ScreenshotDirectory ('setup-{0:D2}.png' -f $script:screenshotNumber)
                $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
                Write-Host "Captured $Label at $path (hosted display, not real 4K validation)."
            } finally { $bitmap.Dispose() }
        } finally { [void][SetupDialogSmoke]::SetThreadDpiAwarenessContext($previousDpi) }
    }
}

$launches = @(
    @{ Name = 'viberwhisper.exe'; Setup = $true; Shell = $false; Ssh = $false },
    @{ Name = 'viberwhisper.exe'; Setup = $true; Shell = $false; Ssh = $false; Realtime = $true },
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
        [SetupDialogSmoke]::CompleteStep($setupProcess, '识别方式', $(if ($launch.Realtime) { '2' } else { '1' }), $false, $false)
        if ($launch.Realtime) {
            [SetupDialogSmoke]::CompleteStep($setupProcess, 'Realtime 服务地址', 'ws://127.0.0.1:8080/v1/realtime', $false, $false)
            [SetupDialogSmoke]::CompleteStep($setupProcess, 'Realtime 模型名称', 'realtime-test', $false, $false)
            [SetupDialogSmoke]::CompleteStep($setupProcess, '环境变量名', '', $false, $true)
        } else {
            if ($launch.Setup) {
                [SetupDialogSmoke]::CompleteStep($setupProcess, 'STT API 地址', '', $false, $false)
                [SetupDialogSmoke]::DismissMessage($setupProcess, '该项不能为空。')
            }
            [SetupDialogSmoke]::CompleteStep($setupProcess, 'STT API 地址', 'https://example.com/v1/audio/transcriptions', $false, $false)
            [SetupDialogSmoke]::CompleteStep($setupProcess, 'STT 模型名称', 'whisper-test', $false, $false)
            [SetupDialogSmoke]::CompleteStep($setupProcess, 'API', '', $true, $true)
        }
        if (!$setupProcess.WaitForExit(10000)) { throw "$binaryName did not exit after setup cancellation" }
        if ($setupProcess.ExitCode -ne 0) { throw "$binaryName returned exit code $($setupProcess.ExitCode)" }
        if (Test-Path $configPath) { throw "$binaryName saved configuration after cancellation" }
        Write-Host "${binaryName}: PMv2 setup windows appeared; cancellation exited without saving (realtime=$([bool]$launch.Realtime))."
    } finally {
        if (!$setupProcess.HasExited) {
            $setupProcess.Kill($true)
            $setupProcess.WaitForExit()
        }
        $setupProcess.Dispose()
    }
}
