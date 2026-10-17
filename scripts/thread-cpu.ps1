# Per-thread CPU of a running process over a window, with thread names
# (GetThreadDescription -- Clippy names its threads).
#   pwsh scripts/thread-cpu.ps1 -ProcessName clippy -Seconds 10
param(
    [string]$ProcessName = 'clippy',
    [int]$Seconds = 10
)
Set-StrictMode -Version Latest
if (-not ('ThreadInfo' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ThreadInfo {
    [DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern int GetThreadDescription(IntPtr h, out IntPtr name);
    [DllImport("kernel32.dll")] public static extern bool GetThreadTimes(IntPtr h, out long c, out long e, out long k, out long u);
    public static string Name(uint id) {
        IntPtr h = OpenThread(0x0800, false, id); // THREAD_QUERY_LIMITED_INFORMATION
        if (h == IntPtr.Zero) return "?";
        try { IntPtr p; if (GetThreadDescription(h, out p) >= 0 && p != IntPtr.Zero) return Marshal.PtrToStringUni(p); return ""; }
        finally { CloseHandle(h); }
    }
    public static double Cpu(uint id) {
        IntPtr h = OpenThread(0x0800, false, id);
        if (h == IntPtr.Zero) return -1;
        try { long c, e, k, u; return GetThreadTimes(h, out c, out e, out k, out u) ? (k + u) / 1e7 : -1; }
        finally { CloseHandle(h); }
    }
}
'@
}
$p = Get-Process -Name $ProcessName -ErrorAction Stop | Select-Object -First 1
$before = @{}
foreach ($t in $p.Threads) { $before[$t.Id] = [ThreadInfo]::Cpu([uint32]$t.Id) }
Start-Sleep -Seconds $Seconds
$rows = foreach ($t in (Get-Process -Id $p.Id).Threads) {
    $now = [ThreadInfo]::Cpu([uint32]$t.Id)
    $was = if ($before.ContainsKey($t.Id)) { $before[$t.Id] } else { 0 }
    if ($now -ge 0) {
        [pscustomobject]@{ Id = $t.Id; Name = [ThreadInfo]::Name([uint32]$t.Id); CpuPct = [math]::Round(100 * ($now - $was) / $Seconds, 2) }
    }
}
$rows | Sort-Object CpuPct -Descending | Select-Object -First 15 | Format-Table -AutoSize
"total: {0:N1} % of one core" -f (($rows | Measure-Object CpuPct -Sum).Sum)
