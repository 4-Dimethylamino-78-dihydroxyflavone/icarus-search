# Run icarus-search in a real classic console window and photograph that
# window (and only that window) to a PNG.
#
#   shoot.ps1 -Exe <path> -Out <png> [-Keys "<script>"] [-Extra "<args>"]
#             [-Cols 172] [-Rows 44] [-Wait 3500]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out,
    [string] $Keys = "",
    [string] $Extra = "--demo",
    [int] $Cols = 0,
    [int] $Rows = 0,
    [int] $Wait = 3500,
    # Start it from a shell (as from a prompt) rather than alone (a double-click).
    [switch] $Shell,
    # Resize the window to this many pixels, without activating it.
    [int] $Width = 0,
    [int] $Height = 0,
    # Leave the window however it is; do not show or resize it.
    [switch] $AsIs,
    # Show the window as soon as it exists and report its size, before the
    # app's start-up script runs. Begin -Keys with "pause" to give this time.
    [switch] $Early,
    # Keep the shell window open after the app quits (end -Keys with "q"),
    # to see the state the app leaves a borrowed console in.
    [switch] $Linger
)

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Shot {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, string cls, string title);
    // Every classic console window on the desktop.
    public static System.Collections.Generic.List<IntPtr> Consoles() {
        var found = new System.Collections.Generic.List<IntPtr>();
        IntPtr at = IntPtr.Zero;
        while ((at = FindWindowExW(IntPtr.Zero, at, "ConsoleWindowClass", null)) != IntPtr.Zero) found.Add(at);
        return found;
    }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
[void][Shot]::SetProcessDPIAware()

$before = @(Get-Process icarus-search -ErrorAction SilentlyContinue | ForEach-Object Id)
# This shell runs with NO_COLOR set, which the terminal library obeys by
# dropping every colour. A double-clicked program inherits no such thing.
Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue
if ($Shell -or $Cols -gt 0) {
    # Through a batch file (which also sidesteps three layers of nested
    # quoting). The shell stays attached to the console, as it would if the
    # program were started from a prompt.
    $batch = Join-Path ([IO.Path]::GetDirectoryName($Out)) "launch.cmd"
    $line = "`"$Exe`" $Extra"
    if ($Keys) { $line += " --keys `"$Keys`"" }
    $script = @("@echo off")
    if ($Cols -gt 0) { $script += "mode con: cols=$Cols lines=$Rows" }
    $script += $line
    if ($Linger) {
        # Stay open after the app quits, to photograph what it left behind.
        $script += @("echo.", "echo BACK AT THE SHELL: this line is in the console's own font.", "timeout /t 8 >nul")
    }
    Set-Content -Path $batch -Encoding ascii -Value $script
    $arguments = "cmd.exe /c `"$batch`""
} else {
    # Directly, so the program is alone on its console: a double-click.
    $arguments = "`"$Exe`" $Extra"
    if ($Keys) { $arguments += " --keys `"$Keys`"" }
}
$existing = @([Shot]::Consoles())
# conhost.exe forces the classic console even where Windows Terminal is the default.
$host_proc = Start-Process conhost.exe -ArgumentList $arguments -WindowStyle Minimized -PassThru

# Find the window: the console that was not there before. (A console window
# reports its first client as its owner, not conhost, so going through the
# process we launched does not work.)
$deadline = (Get-Date).AddSeconds(25)
$hwnd = [IntPtr]::Zero
while ((Get-Date) -lt $deadline -and $hwnd -eq [IntPtr]::Zero) {
    Start-Sleep -Milliseconds 100
    $new = @([Shot]::Consoles() | Where-Object { $existing -notcontains $_ })
    if ($new.Count -gt 0) { $hwnd = $new[0] }
}
if ($Early -and $hwnd -ne [IntPtr]::Zero) {
    # Put it on screen at once, before the app's start-up script runs (begin
    # the script with "pause"), so that what follows happens to a visible
    # window of ordinary size - as it would for a person using it.
    [void][Shot]::ShowWindow($hwnd, 4)
    Start-Sleep -Milliseconds 300
    $rect0 = New-Object Shot+RECT
    [void][Shot]::GetWindowRect($hwnd, [ref]$rect0)
    "window before: $($rect0.Right - $rect0.Left)x$($rect0.Bottom - $rect0.Top)"
}
if ($hwnd -eq [IntPtr]::Zero) {
    "console window never appeared; launcher exited=$($host_proc.HasExited)"
    Get-Process icarus-search -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | Stop-Process -Force
    exit 1
}

# Bring it up without taking the keyboard away from whoever is working.
if (-not $AsIs -and -not $Early) { [void][Shot]::ShowWindow($hwnd, 4) }   # SW_SHOWNOACTIVATE
if ($Width -gt 0) {
    Start-Sleep -Milliseconds 400
    # SWP_NOZORDER | SWP_NOACTIVATE
    [void][Shot]::SetWindowPos($hwnd, [IntPtr]::Zero, 0, 0, $Width, $Height, 0x0014)
}
Start-Sleep -Milliseconds $Wait

$rect = New-Object Shot+RECT
[void][Shot]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
$bmp = New-Object System.Drawing.Bitmap $w, $h
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $gfx.GetHdc()
$ok = [Shot]::PrintWindow($hwnd, $hdc, 2)   # PW_RENDERFULLCONTENT
$gfx.ReleaseHdc($hdc)
$gfx.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

# Close only the copy this script started.
Get-Process icarus-search -ErrorAction SilentlyContinue |
    Where-Object { $before -notcontains $_.Id } |
    Stop-Process -Force
Start-Sleep -Milliseconds 300
if (-not $host_proc.HasExited) { Stop-Process -Id $host_proc.Id -Force -ErrorAction SilentlyContinue }
"saved $Out  ${w}x${h}  printwindow=$ok"
