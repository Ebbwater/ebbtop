# Acceptance helper, run ON the desktop (Windows). No network, no secret.
# Usage:
#   powershell -ExecutionPolicy Bypass -File deploy\acceptance-desktop.ps1 [-LogPath <path>]
# Default log: %LOCALAPPDATA%\ebbtop\ebbtop.log (written by the EbbTop GUI client).
#
# Prints, in order:
#   (a) an overnight-log summary for brief step 7 (per-minute `gaps=`,
#       `reconnects=`, `interval_p95=` lines; `disconnected` + recovery)
#   (b) the btop-accuracy comparison snippet for brief step 4
#   (c) the steps 1-to-7 checklist with how to record each one
#
# Private-network and local: reads one local log file, prints no token (reads
# neither the credential store nor any token file), opens no port, and never
# contacts anything but localhost. The btop comparison is manual on purpose
# (it needs a human glance at both windows); nothing here reaches the agent host.
param([string]$LogPath)
$ErrorActionPreference = 'Stop'
if (-not $LogPath) {
    $LogPath = Join-Path $env:LOCALAPPDATA 'ebbtop\ebbtop.log'
}

Write-Host "== ebbtop.log ($LogPath)"
if (-not (Test-Path $LogPath)) {
    Write-Host "log not found. Launch EbbTop once (Desktop shortcut) and let it run a minute, then re-run."
} else {
    $lines = Get-Content $LogPath
    $sum = @($lines | Where-Object { $_ -match 'conn=' })
    $disc = @($lines | Where-Object { $_ -match 'disconnected' })
    Write-Host ("lines: {0} (summary: {1}, disconnected: {2})" -f $lines.Count, $sum.Count, $disc.Count)
    if ($lines.Count) {
        Write-Host ("span: {0} .. {1}" -f $lines[0].Substring(0, [math]::Min(25, $lines[0].Length)), $lines[-1].Substring(0, [math]::Min(25, $lines[-1].Length)))
    }
    if ($sum.Count) {
        function Field([string]$line, [string]$name) {
            if ($line -match (" " + $name + "=([0-9]+)")) { return [int]$Matches[1] }
            return $null
        }
        $p95 = @($sum | ForEach-Object { Field $_ 'interval_p95' } | Where-Object { $_ -ne $null })
        $gaps = @($sum | ForEach-Object { Field $_ 'gaps' } | Where-Object { $_ -ne $null })
        $recs = @($sum | ForEach-Object { Field $_ 'reconnects' } | Where-Object { $_ -ne $null })
        $p95Max = ($p95 | Measure-Object -Maximum).Maximum
        $p95Bad = @($p95 | Where-Object { $_ -ge 2000 }).Count
        $gapsInc = 0
        for ($i = 1; $i -lt $gaps.Count; $i++) { if ($gaps[$i] -gt $gaps[$i - 1]) { $gapsInc++ } }
        Write-Host ("interval_p95_ms: max={0} last={1} lines_ge_2000={2}" -f $p95Max, $p95[-1], $p95Bad)
        Write-Host ("gaps (cumulative): first={0} last={1} delta={2} lines_with_increase={3}" -f $gaps[0], $gaps[-1], ($gaps[-1] - $gaps[0]), $gapsInc)
        Write-Host ("reconnects (cumulative): first={0} last={1} delta={2}" -f $recs[0], $recs[-1], ($recs[-1] - $recs[0]))
        Write-Host ("last summary: {0}" -f $sum[-1])
        if ($p95Bad -gt 0) {
            Write-Host "VERDICT: REVIEW (some interval_p95 >= 2000 ms; brief pass bar is < 2000)"
        } elseif ($disc.Count -gt 0 -and $gapsInc -eq 0) {
            Write-Host "VERDICT: REVIEW (disconnects without gap increases — check the log by hand)"
        } elseif ($disc.Count -eq 0 -and $gapsInc -gt 0) {
            Write-Host "VERDICT: REVIEW (gaps increased with no disconnected line to explain them)"
        } else {
            Write-Host "VERDICT: PASS (all interval_p95 < 2000 ms; disconnect+recovery is expected per step 7)"
        }
        Write-Host "Linux/macOS twin (same check, for a log copied off the desktop): deploy/parse-ebbtop-log.sh <logfile>"
    } else {
        Write-Host "no summary lines yet — let the client run a minute, then re-run."
    }
}

Write-Host ""
Write-Host "== step 4: btop accuracy (manual, side by side)"
Write-Host "On the agent host (one SSH session, then close it):"
Write-Host "  btop"
Write-Host "btop's default CPU% is the share of the whole machine, like EbbTop."
Write-Host "If you use top instead, its per-process %CPU is per core: divide by 12"
Write-Host "threads (for example 240% in top is 20% the way EbbTop shows it)."
Write-Host "Or run the scripted check on the agent host:"
Write-Host "  ~/ebbtop/deploy/acceptance-kserver.sh 30"
Write-Host "Pass: total CPU and memory within 5 points of EbbTop, and EbbTop sorted"
Write-Host "by CPU% shows the same top process name and PID as btop."

Write-Host ""
Write-Host "== steps 1 to 7 checklist (record one line per step in README Evidence)"
Write-Host "[ ] 1. Reboot the desktop. Click 'EbbTop' on the Desktop. Record: launched or not, any dialog."
Write-Host "[ ] 2. Snap EbbTop to half screen (under 1100 px). Record: stacked layout seen or not."
Write-Host "[ ] 3. Within 10 s: LIVE. Hover age for interval_p95. Record: the value; pass is < 2000 ms (1 s design target)."
Write-Host "[ ] 4. Compare with btop on the agent host (snippet above). Record: CPU/mem deltas and top name/PID match."
Write-Host "[ ] 5. No SSH windows open; still LIVE. Record: yes/no (close every SSH window first)."
Write-Host "[ ] 6. Reboot again; one click; reconnects with no token prompt. Record: reconnected or not, prompt seen or not."
Write-Host "[ ] 7. Overnight: next morning re-run this script (section (a)). Record: VERDICT plus gaps/reconnects/interval_p95 maxima."
