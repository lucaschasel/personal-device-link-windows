param([string]$OutputName = 'personal-device-link-v0151-source.zip')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$outputDirectory = Join-Path $projectRoot 'dist'
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$outputPath = Join-Path $outputDirectory $OutputName
$sourceFiles = @('Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE', '.gitignore', '.gitattributes', '启动 Personal Device Link.command', 'Mac 安装说明.txt') |
    ForEach-Object { Get-Item -LiteralPath (Join-Path $projectRoot $_) }
foreach ($directory in @('src', 'tests', 'docs', 'scripts', '.github')) {
    $sourceFiles += Get-ChildItem -LiteralPath (Join-Path $projectRoot $directory) -File -Recurse -Force
}
$sourceFiles += @('package.json', 'package-lock.json', 'tsconfig.json', 'index.html') |
    ForEach-Object { Get-Item -LiteralPath (Join-Path $projectRoot "desktop/$_") }
$sourceFiles += Get-ChildItem -LiteralPath (Join-Path $projectRoot 'desktop/src') -File -Recurse
$sourceFiles += Get-ChildItem -LiteralPath (Join-Path $projectRoot 'desktop/tests') -File -Recurse
$sourceFiles += @('Cargo.toml', 'Cargo.lock', 'build.rs', 'tauri.conf.json', 'Info.plist') |
    ForEach-Object { Get-Item -LiteralPath (Join-Path $projectRoot "desktop/src-tauri/$_") }
foreach ($directory in @('src', 'icons', 'capabilities', 'examples')) {
    $path = Join-Path $projectRoot "desktop/src-tauri/$directory"
    if (Test-Path -LiteralPath $path) { $sourceFiles += Get-ChildItem -LiteralPath $path -File -Recurse }
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$stream = [System.IO.File]::Open($outputPath, [System.IO.FileMode]::Create)
$archive = [System.IO.Compression.ZipArchive]::new($stream, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in $sourceFiles) {
        $entryName = $file.FullName.Substring($projectRoot.Length + 1).Replace('\', '/')
        $entry = [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $entryName)
        # Executable bits must survive extraction on macOS (including Finder).
        $mode = if ($file.Extension -in @('.command', '.sh')) { 33261 } else { 33188 }
        $entry.ExternalAttributes = [int]($mode -shl 16)
    }
} finally {
    $archive.Dispose()
    $stream.Dispose()
}
# .NET on Windows writes the DOS creator platform. Mark central-directory entries
# as Unix so unzip / Archive Utility honor the POSIX modes above. This archive is
# produced without comments or ZIP64; validate each header before updating it.
$zipFile = [System.IO.File]::Open($outputPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite)
$reader = [System.IO.BinaryReader]::new($zipFile)
try {
    $zipFile.Position = $zipFile.Length - 22
    if ($reader.ReadUInt32() -ne 0x06054b50) { throw 'Unexpected ZIP end record' }
    $zipFile.Position = $zipFile.Length - 12
    $count = $reader.ReadUInt16()
    $zipFile.Position = $zipFile.Length - 6
    $offset = $reader.ReadUInt32()
    for ($i = 0; $i -lt $count; $i++) {
        $zipFile.Position = $offset
        if ($reader.ReadUInt32() -ne 0x02014b50) { throw 'Unexpected ZIP central directory' }
        $zipFile.Position = $offset + 5
        $zipFile.WriteByte(3)
        $zipFile.Position = $offset + 28
        $nameLength = $reader.ReadUInt16()
        $extraLength = $reader.ReadUInt16()
        $commentLength = $reader.ReadUInt16()
        $offset += 46 + $nameLength + $extraLength + $commentLength
    }
} finally {
    $reader.Dispose()
    $zipFile.Dispose()
}
Get-Item -LiteralPath $outputPath | Select-Object FullName, Length
