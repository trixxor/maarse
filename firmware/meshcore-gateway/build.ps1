param(
    [ValidateSet('heltec_v4_kiss_backbone', 'heltec_v4_kiss_backbone_tft')]
    [string[]] $Environment = @('heltec_v4_kiss_backbone', 'heltec_v4_kiss_backbone_tft')
)

$ErrorActionPreference = 'Stop'
$meshCoreRevision = '75b04ef06def54b348400cb1b138afb6749ffd4f'
$meshCoreSource = Join-Path $PSScriptRoot 'src'

if (-not (Test-Path -LiteralPath $meshCoreSource)) {
    git clone --no-checkout -- https://github.com/meshcore-dev/meshcore $meshCoreSource
    if ($LASTEXITCODE -ne 0) { throw 'Could not clone MeshCore.' }
    git -C $meshCoreSource checkout --detach $meshCoreRevision
    if ($LASTEXITCODE -ne 0) { throw 'Could not check out the pinned MeshCore revision.' }
}

$checkedOutRevision = git -C $meshCoreSource rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $checkedOutRevision -ne $meshCoreRevision -or
    -not (Test-Path -LiteralPath (Join-Path $meshCoreSource 'platformio.ini'))) {
    throw "Expected MeshCore revision $meshCoreRevision in $meshCoreSource."
}

Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'platformio.gateway.ini') `
    -Destination (Join-Path $meshCoreSource 'platformio.local.ini')

$pioArguments = @('run', '-d', $meshCoreSource)
foreach ($target in $Environment) { $pioArguments += '-e', $target }
pio @pioArguments
exit $LASTEXITCODE
