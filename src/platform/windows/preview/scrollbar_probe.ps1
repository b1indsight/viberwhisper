# Private feasibility driver, only launched by the isolated native test child. No product UI.
param(
    [Parameter(Mandatory)][long]$Window,
    [Parameter(Mandatory)][long]$Editor,
    [Parameter(Mandatory)][int]$OwnerProcess,
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory)][ValidateSet('native-ready', 'native-readonly', 'ready', 'readonly')][string]$Case,
    [Parameter(Mandatory)][int]$ThumbWidth
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase, System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class ScrollbarInput {
    public struct Point { public int X, Y; }
    public struct Rect { public int Left, Top, Right, Bottom; }
    public struct Scrollbar {
        public uint Size;
        public Rect Bounds;
        public int ArrowSize, ThumbTop, ThumbBottom, Reserved;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst=6)] public uint[] State;
    }
    public struct GuiThread {
        public uint Size, Flags;
        public IntPtr Active, Focus, Capture, MenuOwner, MoveSize, Caret;
        public Rect CaretBounds;
    }
    public struct ScrollInfo {
        public uint Size, Mask;
        public int Minimum, Maximum;
        public uint Page;
        public int Position, TrackPosition;
    }
    public struct Mouse { public int X, Y; public uint Data, Flags, Time; public UIntPtr Extra; }
    public struct Key { public ushort Code, Scan; public uint Flags, Time; public UIntPtr Extra; }
    [StructLayout(LayoutKind.Explicit)] public struct Union {
        [FieldOffset(0)] public Mouse Mouse;
        [FieldOffset(0)] public Key Key;
    }
    public struct Input { public uint Type; public Union Data; }
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect bounds);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window, ref Point point);
    [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr window, IntPtr region);
    [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int left, int top, int right, int bottom);
    [DllImport("gdi32.dll")] public static extern bool PtInRegion(IntPtr region, int x, int y);
    [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr value);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern uint GetDoubleClickTime();
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr window, uint flags);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern bool GetScrollBarInfo(IntPtr window, int kind, ref Scrollbar info);
    [DllImport("user32.dll")] public static extern bool GetScrollInfo(IntPtr window, int kind, ref ScrollInfo info);
    [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint thread, ref GuiThread info);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint SendInput(uint count, Input[] inputs, int size);
    [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW", CharSet=CharSet.Unicode)]
    private static extern IntPtr ReadMessage(IntPtr window, uint message, UIntPtr argument, StringBuilder text, uint flags, uint timeout, out UIntPtr result);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
    public static int FirstLine(IntPtr window) {
        UIntPtr result;
        if (ReadMessage(window, 0x00CE, UIntPtr.Zero, null, 2, 500, out result) == IntPtr.Zero)
            throw new Exception("EDIT did not answer EM_GETFIRSTVISIBLELINE");
        return (int)result.ToUInt32();
    }
    public static uint Selection(IntPtr window) {
        UIntPtr result;
        if (ReadMessage(window, 0x00B0, UIntPtr.Zero, null, 2, 500, out result) == IntPtr.Zero)
            throw new Exception("EDIT did not answer EM_GETSEL");
        return result.ToUInt32();
    }
    public static string Text(IntPtr window) {
        var text = new StringBuilder(8192);
        UIntPtr result;
        if (ReadMessage(window, 0x000D, (UIntPtr)text.Capacity, text, 2, 500, out result) == IntPtr.Zero)
            throw new Exception("EDIT did not answer WM_GETTEXT");
        return text.ToString();
    }
    public static void MouseEvent(uint flags, int data) {
        var inputs = new[] { new Input { Type=0, Data=new Union { Mouse=new Mouse { Flags=flags, Data=unchecked((uint)data) } } } };
        if (SendInput(1, inputs, Marshal.SizeOf(typeof(Input))) != 1) throw new Exception("Mouse SendInput failed");
    }
    public static void Keys(ushort code, bool control) {
        uint extended = code == 0x24 || code == 0x22 ? 1u : 0u;
        var down = new Input { Type=1, Data=new Union { Key=new Key { Code=code, Flags=extended } } };
        var up = new Input { Type=1, Data=new Union { Key=new Key { Code=code, Flags=extended|2 } } };
        var inputs = control ? new[] {
            new Input { Type=1, Data=new Union { Key=new Key { Code=0x11 } } }, down, up,
            new Input { Type=1, Data=new Union { Key=new Key { Code=0x11, Flags=2 } } }
        } : new[] { down, up };
        if (SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(Input))) != inputs.Length)
            throw new Exception("Keyboard SendInput failed");
    }
}
'@

$windowHandle = [IntPtr]::new($Window)
$editorHandle = [IntPtr]::new($Editor)
[uint32]$process = 0
$uiThread = [ScrollbarInput]::GetWindowThreadProcessId($windowHandle, [ref]$process)
if ($process -ne $OwnerProcess) { throw 'Preview is not owned by the isolated fixture' }
[void][ScrollbarInput]::GetWindowThreadProcessId($editorHandle, [ref]$process)
if ($process -ne $OwnerProcess) { throw 'EDIT is not owned by the isolated fixture' }
if ([ScrollbarInput]::SetThreadDpiAwarenessContext([IntPtr]::new(-4)) -eq [IntPtr]::Zero) { throw 'Driver could not enter PMv2' }
if ([ScrollbarInput]::GetDpiForWindow($windowHandle) -ne 96) { throw 'This screen gate requires the hosted 96-DPI display; not a 4K test' }

function Get-Scrollbar {
    $info = [ScrollbarInput+Scrollbar]::new()
    $info.Size = [Runtime.InteropServices.Marshal]::SizeOf($info)
    $info.State = [uint32[]]::new(6)
    if (![ScrollbarInput]::GetScrollBarInfo($editorHandle, -5, [ref]$info)) { throw 'No native vertical scrollbar' }
    return $info
}
function Wait-Until([scriptblock]$Check, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(2)
    while (!( & $Check )) {
        if ([DateTime]::UtcNow -ge $deadline) { throw "Timed out: $Label" }
        Start-Sleep -Milliseconds 15
    }
}
function Move-Pointer([int]$X, [int]$Y) {
    if (![ScrollbarInput]::SetCursorPos($X, $Y)) { throw 'Could not move the real pointer' }
    $point = [ScrollbarInput+Point]::new()
    if (![ScrollbarInput]::GetCursorPos([ref]$point) -or $point.X -ne $X -or $point.Y -ne $Y) { throw 'Pointer coordinates differ' }
    if ([ScrollbarInput]::GetAncestor([ScrollbarInput]::WindowFromPoint($point), 2) -ne $windowHandle) { throw 'Another window covers the input point' }
}
function Reset-Scroll {
    [ScrollbarInput]::Keys(0x24, $true) # Real Ctrl+Home, not a scroll message.
    Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -eq 0 -and [ScrollbarInput]::Selection($editorHandle) -eq 0 } 'Ctrl+Home'
}
function Get-Focus {
    $info = [ScrollbarInput+GuiThread]::new()
    $info.Size = [Runtime.InteropServices.Marshal]::SizeOf($info)
    if (![ScrollbarInput]::GetGUIThreadInfo($uiThread, [ref]$info)) { throw 'Cannot read native focus/capture' }
    return $info
}
function Get-Uia([int]$X, [int]$Y) {
    $hit = [Windows.Automation.AutomationElement]::FromPoint([Windows.Point]::new($X, $Y))
    $kind = $hit.Current.ControlType.Id
    $owned = $false
    $walker = [Windows.Automation.TreeWalker]::RawViewWalker
    for ($depth = 0; $null -ne $hit -and $depth -lt 16; $depth++) {
        if ($hit.Current.NativeWindowHandle -eq $Editor) { $owned = $true; break }
        $hit = $walker.GetParent($hit)
    }
    return [ordered]@{ OwnedByEdit=$owned; Type=$kind }
}
function Test-Slot($Bitmap, $Info, [int]$Color) {
    $left = $Info.Bounds.Left - $script:bounds.Left
    $top = $Info.Bounds.Top - $script:bounds.Top
    $width = $Info.Bounds.Right - $Info.Bounds.Left
    $height = $Info.Bounds.Bottom - $Info.Bounds.Top
    $middle = $top + [int](($Info.ThumbTop + $Info.ThumbBottom) / 2)
    $painted = 0
    $firstPainted = $height
    $lastPainted = -1
    for ($y = 0; $y -lt $height; $y++) {
        for ($x = 0; $x -lt $width; $x++) {
            $rgb = $Bitmap.GetPixel($left + $x, $top + $y).ToArgb() -band 0xffffff
            if ($rgb -ne 0x1e1e1e -and $rgb -ne $Color) { return $false }
            if ($rgb -eq $Color -and ($y -lt $Info.ThumbTop -or $y -ge $Info.ThumbBottom)) { return $false }
            if ($rgb -eq $Color) { $firstPainted = [Math]::Min($firstPainted, $y); $lastPainted = $y }
            if ($top + $y -eq $middle -and $rgb -eq $Color) { $painted++ }
        }
    }
    return $painted -eq $ThumbWidth -and [Math]::Abs($firstPainted - $Info.ThumbTop) -le 1 -and [Math]::Abs($lastPainted - ($Info.ThumbBottom - 1)) -le 1
}
function Get-CaretBounds {
    $gui = Get-Focus
    if ($gui.Caret -eq [IntPtr]::Zero) { return [ordered]@{ Left=0; Top=0; Right=0; Bottom=0 } }
    $point = [ScrollbarInput+Point]::new()
    if (![ScrollbarInput]::ClientToScreen($gui.Caret, [ref]$point)) { throw 'Cannot locate native caret' }
    return [ordered]@{
        Left=$gui.CaretBounds.Left + $point.X - $script:bounds.Left - 1
        Top=$gui.CaretBounds.Top + $point.Y - $script:bounds.Top - 1
        Right=$gui.CaretBounds.Right + $point.X - $script:bounds.Left + 1
        Bottom=$gui.CaretBounds.Bottom + $point.Y - $script:bounds.Top + 1
    }
}
function Compare-OutsideSlot($Bitmap, $Info, [string]$Label, $Caret) {
    $path = Join-Path $Directory "probe-screen-$baseCase-$Label.png"
    $original = [Drawing.Bitmap]::new($path)
    try {
        $oldCaret = Get-Content -Raw "$path.caret.json" | ConvertFrom-Json
        if ($original.Width -ne $Bitmap.Width -or $original.Height -ne $Bitmap.Height) { throw 'Overlay changed screen bounds' }
        for ($y = 0; $y -lt $Bitmap.Height; $y++) {
            for ($x = 0; $x -lt $Bitmap.Width; $x++) {
                if (![ScrollbarInput]::PtInRegion($region, $x, $y)) { continue }
                if ($x + $script:bounds.Left -ge $Info.Bounds.Left -and $x + $script:bounds.Left -lt $Info.Bounds.Right -and
                    $y + $script:bounds.Top -ge $Info.Bounds.Top -and $y + $script:bounds.Top -lt $Info.Bounds.Bottom) { continue }
                # A blinking caret is the only permitted difference inside the owned window.
                if (($x -ge $Caret.Left -and $x -lt $Caret.Right -and $y -ge $Caret.Top -and $y -lt $Caret.Bottom) -or
                    ($x -ge $oldCaret.Left -and $x -lt $oldCaret.Right -and $y -ge $oldCaret.Top -and $y -lt $oldCaret.Bottom)) { continue }
                if ($original.GetPixel($x, $y).ToArgb() -ne $Bitmap.GetPixel($x, $y).ToArgb()) { throw "Screen changed outside the slot at $x,$y ($Label)" }
            }
        }
    } finally { $original.Dispose() }
}
function Capture-Screen([string]$Label, [int]$Color) {
    $deadline = [DateTime]::UtcNow.AddSeconds(2)
    $previous = $null
    do {
        if ([ScrollbarInput]::DwmFlush() -ne 0) { throw 'Desktop composition did not flush' }
        $bitmap = [Drawing.Bitmap]::new($script:bounds.Right - $script:bounds.Left, $script:bounds.Bottom - $script:bounds.Top)
        try {
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try { $graphics.CopyFromScreen($script:bounds.Left, $script:bounds.Top, 0, 0, $bitmap.Size) }
            finally { $graphics.Dispose() }
            $info = Get-Scrollbar
            $matches = Test-Slot $bitmap $info $Color
            $path = Join-Path $Directory "probe-screen-$Case-$Label.png"
            $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
            $caret = Get-CaretBounds
            if ($native) {
                if ($matches) { throw 'Negative control: native scrollbar unexpectedly passed the custom screen gate' }
                $caret | ConvertTo-Json | Set-Content -Encoding UTF8 "$path.caret.json"
                return
            }
            # Require two consecutive matching rail/thumb captures without forcing any repaint.
            $state = "${matches}:$($info.ThumbTop):$($info.ThumbBottom)"
            if ($matches -and $state -eq $previous) {
                Compare-OutsideSlot $bitmap $info $Label $caret
                return
            }
            $previous = $state
        } finally { $bitmap.Dispose() }
        Start-Sleep -Milliseconds 15
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Actual screen did not show the narrow $Label thumb; last frame saved"
}

$script:bounds = [ScrollbarInput+Rect]::new()
if (![ScrollbarInput]::GetWindowRect($windowHandle, [ref]$script:bounds)) { throw 'Cannot measure owned preview' }
$cursor = [ScrollbarInput+Point]::new()
if (![ScrollbarInput]::GetCursorPos([ref]$cursor)) { throw 'Cannot save pointer position' }
$originalText = [ScrollbarInput]::Text($editorHandle)
$metrics = [ordered]@{}
$native = $Case.StartsWith('native-')
$baseCase = if ($Case.EndsWith('readonly')) { 'native-readonly' } else { 'native-ready' }
$baselinePath = Join-Path $Directory "probe-input-$baseCase.json"
$region = [ScrollbarInput]::CreateRectRgn(0, 0, 0, 0)
try {
    if ($region -eq [IntPtr]::Zero -or [ScrollbarInput]::GetWindowRgn($windowHandle, $region) -eq 0) { throw 'Cannot read owned window region' }
    $scroll = [ScrollbarInput+ScrollInfo]::new()
    $scroll.Size = [Runtime.InteropServices.Marshal]::SizeOf($scroll)
    $scroll.Mask = 0x17
    if (![ScrollbarInput]::GetScrollInfo($editorHandle, 1, [ref]$scroll) -or $scroll.Page -eq 0) { throw 'Cannot read native scroll range/page' }
    # Expected endpoint comes from Win32, independently of the product's viewport helper.
    $maximumFirstLine = $scroll.Maximum - $scroll.Page + 1
    # A client click establishes real focus; the overlay must never acquire it.
    $bar = Get-Scrollbar
    Move-Pointer ($bar.Bounds.Left - 30) ($bar.Bounds.Top + 10)
    [ScrollbarInput]::MouseEvent(2, 0)
    [ScrollbarInput]::MouseEvent(4, 0)
    Wait-Until { (Get-Focus).Focus -eq $editorHandle -and [ScrollbarInput]::GetForegroundWindow() -eq $windowHandle } 'EDIT focus'
    Reset-Scroll
    $bar = Get-Scrollbar
    $middle = $bar.Bounds.Top + [int](($bar.ThumbTop + $bar.ThumbBottom) / 2)
    $metrics.UiaEdge = Get-Uia ($bar.Bounds.Left + 1) $middle
    $metrics.UiaThumb = Get-Uia ([int](($bar.Bounds.Left + $bar.Bounds.Right) / 2)) $middle
    $outside = Get-Uia ($script:bounds.Left + 20) ($script:bounds.Top + 2)
    if ($outside.OwnedByEdit) { throw 'Negative control: UIA accepted a point outside EDIT' }
    $element = [Windows.Automation.AutomationElement]::FromHandle($editorHandle)
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::ScrollBar)
    $accessibleBar = $element.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
    $metrics.AccessibleScrollbar = $null -ne $accessibleBar -and !$accessibleBar.Current.IsOffscreen
    $metrics.Patterns = @($element.GetSupportedPatterns() | ForEach-Object { $_.Id } | Sort-Object)
    $children = @()
    $walker = [Windows.Automation.TreeWalker]::ControlViewWalker
    $child = $walker.GetFirstChild([Windows.Automation.AutomationElement]::FromHandle($windowHandle))
    while ($null -ne $child) {
        $children += "$($child.Current.ControlType.Id):$($child.Current.NativeWindowHandle)"
        $child = $walker.GetNextSibling($child)
    }
    $metrics.ControlChildren = @($children | Sort-Object)
    Write-Output ("scrollbar UIA $Case : " + ($metrics | ConvertTo-Json -Depth 4 -Compress))
    Capture-Screen 'idle' 0x929292

    # Compare exact native responses, not merely nonzero motion (which can hide double scrolling).
    foreach ($location in @('client', 'slot')) {
        Reset-Scroll
        $bar = Get-Scrollbar
        $x = if ($location -eq 'slot') { $bar.Bounds.Left + 1 } else { $bar.Bounds.Left - 30 }
        Move-Pointer $x ($bar.Bounds.Top + 100)
        [ScrollbarInput]::MouseEvent(0x800, -120)
        Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -gt 0 } "wheel $location"
        $metrics["Wheel-$location"] = [ScrollbarInput]::FirstLine($editorHandle)
    }
    Reset-Scroll
    [ScrollbarInput]::Keys(0x22, $false)
    Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -gt 0 } 'PageDown'
    $metrics.PageDown = [ScrollbarInput]::FirstLine($editorHandle)
    Reset-Scroll
    $bar = Get-Scrollbar
    Move-Pointer ($bar.Bounds.Left + 1) ($bar.Bounds.Top + $bar.ThumbBottom + 8)
    [ScrollbarInput]::MouseEvent(2, 0)
    [ScrollbarInput]::MouseEvent(4, 0)
    Wait-Until { (Get-Focus).Capture -eq [IntPtr]::Zero -and [ScrollbarInput]::FirstLine($editorHandle) -gt 0 } 'native track click'
    $metrics.TrackClick = [ScrollbarInput]::FirstLine($editorHandle)
    Reset-Scroll

    # Native tracking runs inside DefSubclassProc's modal loop while this external client moves
    # and captures. No WM_VSCROLL/WM_NCLBUTTONDOWN messages are manufactured by this driver.
    Start-Sleep -Milliseconds ([ScrollbarInput]::GetDoubleClickTime() + 1)
    $bar = Get-Scrollbar
    $x = [int](($bar.Bounds.Left + $bar.Bounds.Right) / 2)
    Move-Pointer $x ($bar.Bounds.Top + [int](($bar.ThumbTop + $bar.ThumbBottom) / 2))
    Capture-Screen 'hover' 0xb0b0b0
    [ScrollbarInput]::MouseEvent(2, 0)
    Wait-Until { (Get-Focus).Capture -eq $editorHandle } 'native thumb capture'
    Capture-Screen 'pressed' 0xc7c7c7
    Move-Pointer $x ([int](($bar.Bounds.Top + $bar.Bounds.Bottom) / 2))
    Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -gt 0 -and [ScrollbarInput]::FirstLine($editorHandle) -lt $maximumFirstLine } 'drag through middle'
    Capture-Screen 'drag-middle' 0xc7c7c7
    Move-Pointer $x ($bar.Bounds.Bottom - 1)
    Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -eq $maximumFirstLine } 'drag to bottom'
    Capture-Screen 'drag-bottom' 0xc7c7c7
    $metrics.DragBottom = [ScrollbarInput]::FirstLine($editorHandle)
    Move-Pointer $x ($bar.Bounds.Top + 1)
    Wait-Until { [ScrollbarInput]::FirstLine($editorHandle) -eq 0 } 'drag to top'
    Capture-Screen 'drag-top' 0xc7c7c7
    [ScrollbarInput]::MouseEvent(4, 0)
    Wait-Until { (Get-Focus).Capture -eq [IntPtr]::Zero } 'native thumb release'
    Capture-Screen 'released' 0xb0b0b0
    Move-Pointer ($bar.Bounds.Left - 30) ($bar.Bounds.Top + 100)
    Capture-Screen 'leave' 0x929292
    $bar = Get-Scrollbar
    $metrics.UiaAfter = Get-Uia ($bar.Bounds.Left + 1) ($bar.Bounds.Top + [int](($bar.ThumbTop + $bar.ThumbBottom) / 2))
    $metrics.Selection = [ScrollbarInput]::Selection($editorHandle)
    if ([ScrollbarInput]::Text($editorHandle) -cne $originalText -or (Get-Focus).Focus -ne $editorHandle) { throw 'Input lost EDIT text or focus' }
    if (!$metrics.UiaEdge.OwnedByEdit -or !$metrics.UiaThumb.OwnedByEdit -or !$metrics.UiaAfter.OwnedByEdit -or !$metrics.AccessibleScrollbar) { throw 'UIA did not preserve native EDIT/scrollbar ownership' }
    if ($native) {
        $metrics | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 $baselinePath
    } else {
        $baseline = Get-Content -Raw $baselinePath | ConvertFrom-Json
        foreach ($key in $metrics.Keys) {
            if (($metrics[$key] | ConvertTo-Json -Depth 4 -Compress) -cne ($baseline.$key | ConvertTo-Json -Depth 4 -Compress)) { throw "Overlay changed native input/UIA result: $key" }
        }
    }
    Write-Output ("scrollbar desktop $Case passed (actual DPI=96, no 4K claim): " + ($metrics | ConvertTo-Json -Depth 4 -Compress))
} finally {
    # Every operation after LEFTDOWN is bounded and no UIA request runs with a held button.
    try { [ScrollbarInput]::MouseEvent(4, 0) }
    finally {
        [void][ScrollbarInput]::SetCursorPos($cursor.X, $cursor.Y)
        [void][ScrollbarInput]::DeleteObject($region)
    }
}
