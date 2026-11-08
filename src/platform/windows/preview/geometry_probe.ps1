# Owned WPF/UIA target and solid desktop backdrop; supervised by the native test's job.
param(
    [Parameter(Mandatory)][string]$Protocol,
    [Parameter(Mandatory)][string]$Directory
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase, System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class GeometryScreen {
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern bool DestroyCaret();
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] private static extern int GetWindowRgn(IntPtr window, IntPtr region);
    [DllImport("gdi32.dll")] private static extern IntPtr CreateRectRgn(int left, int top, int right, int bottom);
    [DllImport("gdi32.dll")] private static extern bool PtInRegion(IntPtr region, int x, int y);
    [DllImport("gdi32.dll")] private static extern bool DeleteObject(IntPtr region);
    [DllImport("dwmapi.dll")] private static extern int DwmFlush();
    public static string Capture(string directory, string label, long window, int[] bounds, int[] capture, int[][] excluded) {
        if (DwmFlush() != 0) throw new Exception("DWM flush failed");
        var region = CreateRectRgn(0, 0, 0, 0);
        try {
            if (region == IntPtr.Zero || GetWindowRgn(new IntPtr(window), region) == 0)
                throw new Exception("Missing preview region");
            using (var expected = new Bitmap(System.IO.Path.Combine(directory, "geometry-expected-" + label + ".png")))
            using (var actual = new Bitmap(capture[2] - capture[0], capture[3] - capture[1], PixelFormat.Format24bppRgb)) {
                using (var graphics = Graphics.FromImage(actual))
                    graphics.CopyFromScreen(capture[0], capture[1], 0, 0, actual.Size);
                // Keep the raw old-union-new desktop frame even when an assertion fails.
                actual.Save(System.IO.Path.Combine(directory, "geometry-screen-" + label + ".png"), ImageFormat.Png);
                for (int y = 0; y < actual.Height; y++) {
                    for (int x = 0; x < actual.Width; x++) {
                        int sx = capture[0] + x, sy = capture[1] + y;
                        bool skip = false;
                        foreach (var child in excluded)
                            if (sx >= child[0] && sx < child[2] && sy >= child[1] && sy < child[3]) skip = true;
                        if (skip) continue;
                        int wx = sx - bounds[0], wy = sy - bounds[1];
                        int wanted = wx >= 0 && wy >= 0 && wx < expected.Width && wy < expected.Height && PtInRegion(region, wx, wy)
                            ? expected.GetPixel(wx, wy).ToArgb() : Color.FromArgb(0x22, 0x66, 0x44).ToArgb();
                        if (actual.GetPixel(x, y).ToArgb() != wanted)
                            return "Desktop/chrome mismatch at " + sx + "," + sy + " (" + label + ")";
                    }
                }
                return null;
            }
        } finally { if (region != IntPtr.Zero) DeleteObject(region); }
    }
}
'@
[void][GeometryScreen]::SetThreadDpiAwarenessContext([IntPtr]::new(-4))

function Pump-Desktop {
    $frame = [Windows.Threading.DispatcherFrame]::new()
    $exitFrame = [Action]({ $frame.Continue = $false }.GetNewClosure())
    [void][Windows.Threading.Dispatcher]::CurrentDispatcher.BeginInvoke(
        [Windows.Threading.DispatcherPriority]::Background, $exitFrame)
    [Windows.Threading.Dispatcher]::PushFrame($frame)
}
function Wait-File([string]$Name) {
    while (!(Test-Path (Join-Path $Protocol $Name))) { Pump-Desktop; Start-Sleep -Milliseconds 5 }
}
function Write-Target([string]$Phase) {
    Pump-Desktop
    $origin = $text.PointToScreen([Windows.Point]::new(0, 0))
    $scale = [Windows.PresentationSource]::FromVisual($text).CompositionTarget.TransformToDevice
    $caret = $text.GetRectFromCharacterIndex($text.CaretIndex, $false)
    $caretOrigin = $text.PointToScreen($caret.TopLeft)
    $record = @{
        window = $handle.ToInt64()
        input = @($origin.X, $origin.Y, ($text.ActualWidth * $scale.M11), ($text.ActualHeight * $scale.M22))
        caret = @($caretOrigin.X, $caretOrigin.Y, ($caret.Width * $scale.M11), ($caret.Height * $scale.M22))
    } | ConvertTo-Json -Compress
    # Force the reported missing-Win32-caret case without changing WPF's text/selection.
    [void][GeometryScreen]::DestroyCaret()
    $path = Join-Path $Protocol "$Phase.json"
    [IO.File]::WriteAllText("$path.tmp", $record)
    [IO.File]::Move("$path.tmp", $path)
    Wait-File "$Phase.done"
}

$window = [Windows.Window]::new()
$window.Title = 'ViberWhisper owned geometry fixture'
$window.WindowStyle = [Windows.WindowStyle]::None
$window.ResizeMode = [Windows.ResizeMode]::NoResize
$window.WindowState = [Windows.WindowState]::Maximized
$window.Background = [Windows.Media.BrushConverter]::new().ConvertFromString('#226644')
$canvas = [Windows.Controls.Canvas]::new()
$window.Content = $canvas
$text = [Windows.Controls.TextBox]::new()
$text.Width = 240; $text.Height = 180; $text.FontSize = 18
$text.AcceptsReturn = $true
$text.Text = "first line`nsecond line`nthird line`nfourth line`nlast line"
[Windows.Controls.Canvas]::SetLeft($text, 100)
[Windows.Controls.Canvas]::SetTop($text, 80)
[void]$canvas.Children.Add($text)
$button = [Windows.Controls.Button]::new()
$button.Content = 'Non-editable control'; $button.Width = 180; $button.Height = 30
[Windows.Controls.Canvas]::SetLeft($button, 400)
[Windows.Controls.Canvas]::SetTop($button, 80)
[void]$canvas.Children.Add($button)
$password = [Windows.Controls.PasswordBox]::new()
$password.Width = 180; $password.Height = 30
[Windows.Controls.Canvas]::SetLeft($password, 400)
[Windows.Controls.Canvas]::SetTop($password, 140)
[void]$canvas.Children.Add($password)
$combo = [Windows.Controls.ComboBox]::new()
$combo.Width = 180; $combo.Height = 30; $combo.IsEditable = $false
[void]$combo.Items.Add('Selection only'); $combo.SelectedIndex = 0
[Windows.Controls.Canvas]::SetLeft($combo, 400)
[Windows.Controls.Canvas]::SetTop($combo, 200)
[void]$canvas.Children.Add($combo)
try {
    $window.Show()
    $handle = [Windows.Interop.WindowInteropHelper]::new($window).Handle
    [void]$window.Activate()
    $away = $window.PointToScreen([Windows.Point]::new($window.ActualWidth - 40, 40))
    if (![GeometryScreen]::SetCursorPos([int]$away.X, [int]$away.Y)) { throw 'Cannot move pointer within owned backdrop' }
    [void]$text.Focus(); $text.CaretIndex = 3
    Write-Target 'first-line'
    $text.CaretIndex = $text.Text.Length - 2
    Write-Target 'last-line'
    $text.IsReadOnly = $true
    Write-Target 'read-only'
    [void]$button.Focus()
    Write-Target 'button'
    [void]$combo.Focus()
    Write-Target 'combo'
    [void]$password.Focus()
    Write-Target 'password'
    $canvas.Children.Clear()
    Pump-Desktop
    [IO.File]::WriteAllText((Join-Path $Protocol 'backdrop.ready'), '')
    foreach ($corner in 0..1) {
        foreach ($index in 0..6) {
            $label = "$corner-$index"
            Wait-File "frame-$label.json"
            $request = Get-Content -Raw (Join-Path $Protocol "frame-$label.json") | ConvertFrom-Json
            $deadline = [DateTime]::UtcNow.AddSeconds(2)
            $matched = $false
            $stable = $false
            do {
                Pump-Desktop
                $failure = [GeometryScreen]::Capture($Directory, $label, $request.window,
                    [int[]]$request.bounds, [int[]]$request.capture, [int[][]]$request.excluded)
                if ($null -eq $failure -and $matched) { $stable = $true; break }
                $matched = $null -eq $failure
                Start-Sleep -Milliseconds 15
            } while ([DateTime]::UtcNow -lt $deadline)
            if (!$stable) { throw "Desktop frame did not stabilize: $failure" }
            [Console]::Error.WriteLine("geometry desktop frame $label matched; old union new region verified")
            [IO.File]::WriteAllText((Join-Path $Protocol "frame-$label.done"), '')
        }
    }
    Wait-File 'finish'
} finally { $window.Close() }
