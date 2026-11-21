param(
    # Path to the XML report appcert.exe wrote.
    [Parameter(Mandatory = $true)][string]$ReportPath
)
# Read a Windows App Certification Kit report and decide whether anything
# blocking failed.
#
# The report schema (appcert.exe, Windows SDK):
#
#   <REPORT OVERALL_RESULT="PASS|WARNING|FAIL">
#     <REQUIREMENTS>
#       <REQUIREMENT NUMBER TITLE RATIONALE>
#         <TEST INDEX NAME DESCRIPTION EXECUTIONTIME OPTIONAL="TRUE|FALSE">
#           <MESSAGES><MESSAGE TEXT="..." /></MESSAGES>
#           <RESULT><![CDATA[PASS|WARNING|FAIL]]></RESULT>
#
# Result lives in a child element (CDATA), not an attribute, and OPTIONAL is an
# attribute on TEST: a FAIL on an optional test is a finding, not a blocker.
# Exit 0 when no non-optional test failed, 1 when one did, 2 when the report
# carries no test rows at all (a usage or environment problem, not a verdict).
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ReportPath)) {
    Write-Host "no WACK report at $ReportPath"
    exit 2
}
[xml]$r = Get-Content -Raw -LiteralPath $ReportPath
$overall = $r.REPORT.OVERALL_RESULT
Write-Host "overall: $overall"

$tests = @($r.SelectNodes('/REPORT/REQUIREMENTS/REQUIREMENT/TEST'))
if ($tests.Count -eq 0) {
    Write-Host 'the WACK report carries no TEST rows'
    exit 2
}

$rows = foreach ($t in $tests) {
    $res = if ($t.RESULT) { $t.RESULT.InnerText.Trim() } else { 'UNKNOWN' }
    $msgs = @($t.MESSAGES.MESSAGE | ForEach-Object { $_.TEXT }) -join ' | '
    [pscustomobject]@{
        Test     = $t.NAME
        Result   = $res
        Optional = ("$($t.OPTIONAL)" -eq 'TRUE')
        Details  = $msgs
    }
}

$rows | ForEach-Object {
    '{0,-8} {1,-28} optional={2,-5} {3}' -f $_.Result, $_.Test, $_.Optional, $_.Details
}

$pass     = @($rows | Where-Object Result -eq 'PASS').Count
$warn     = @($rows | Where-Object Result -eq 'WARNING').Count
$fail     = @($rows | Where-Object Result -eq 'FAIL')
$blocking = @($fail | Where-Object { -not $_.Optional })
$optional = @($fail | Where-Object { $_.Optional })

Write-Host ''
Write-Host ("{0} tests: {1} PASS, {2} WARNING, {3} FAIL ({4} blocking, {5} optional)" -f `
    $rows.Count, $pass, $warn, $fail.Count, $blocking.Count, $optional.Count)

if ($optional.Count -gt 0) {
    Write-Host ''
    Write-Host 'optional FAILs (findings, not blockers):'
    $optional | ForEach-Object { "  $($_.Test): $($_.Details)" }
}
if ($blocking.Count -gt 0) {
    Write-Host ''
    Write-Host 'BLOCKING FAILs:'
    $blocking | ForEach-Object { "  $($_.Test): $($_.Details)" }
    exit 1
}
Write-Host ''
Write-Host 'no blocking FAIL rows'
exit 0
