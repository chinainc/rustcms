# rustcms release packer
#
# 一键完成：编译（Windows / Linux）→ 组装安装包与升级包 → 归档（zip / tar.gz）
#          → 生成 version.json 与 SHA256SUMS.txt → 输出在线安装/卸载脚本。
#
# 用法（在仓库任意目录执行均可）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File deploy\release\pack-release.ps1
#     -Version 0.2.0       指定版本（缺省读 Cargo.toml 的 version）
#     -Mirror https://...  写进在线安装脚本的镜像地址
#     -SkipLinux           跳过 Linux 交叉编译与 Linux 产物
#     -SkipBuild           复用 target\ 下已有二进制，不重新编译
#     -OutDir D:\publish   换输出目录（默认 E:\CodeBuddy\rustcms发布）
#     -Force               允许覆盖同版本产物
#
# 归档不依赖 Compress-Archive（PS5.1 会写反斜杠路径）与系统 tar.exe
# （Windows 打出来全是 0644，Linux 二进制会丢执行位）：
# zip 用 .NET ZipArchive 逐条写入，tar.gz 手写 ustar 头 + GZipStream。
param(
  [string]$Version = '',
  [string]$Mirror  = '',
  [switch]$SkipLinux,
  [switch]$SkipBuild,
  [switch]$Force,
  [string]$OutDir  = ''
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargo = 'C:\Users\admin\.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

# ---------------------------------------------------------------- 版本号
$cargoToml = [IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
$m = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $m.Success) { throw 'Cargo.toml 里找不到 version' }
$ver = if ($Version) { $Version } else { $m.Groups[1].Value }
if ($ver -notmatch '^\d+\.\d+\.\d+') { throw ('版本号格式不对：' + $ver) }

$out = if ($OutDir) { $OutDir } else { 'E:\CodeBuddy\rustcms发布' }
if (-not (Test-Path $out)) { New-Item -ItemType Directory -Path $out -Force | Out-Null }
$out = (Resolve-Path $out).Path

$winZip = Join-Path $out ('rustcms-v' + $ver + '-windows.zip')
if ((Test-Path $winZip) -and -not $Force) {
  throw ('版本 ' + $ver + ' 已打过包；确认要重打请加 -Force')
}

$mirror = if ($Mirror) { $Mirror.TrimEnd('/') } else { 'https://your-mirror.example.com' }
$stage  = Join-Path $env:TEMP ('rustcms-pack-' + $ver)
$note = 'RustCMS ' + $ver + ' 发行包：编译产物、模板、静态资源与在线升级能力。'
$sqlList = @()
$sqlDir = Join-Path $root 'deploy\sql'
if (Test-Path $sqlDir) {
  $sqlList = @(Get-ChildItem $sqlDir -File -Filter '*.sql' | Sort-Object Name | ForEach-Object { 'deploy/sql/' + $_.Name })
}
$vjSimple = [ordered]@{
  version = $ver
  date    = (Get-Date -Format 'yyyy-MM-dd')
  note    = $note
  sql     = $sqlList
}
$vjSimpleTxt = ($vjSimple | ConvertTo-Json -Depth 5)
if ($vjSimpleTxt -match '"sql":\s*null') { $vjSimpleTxt = $vjSimpleTxt -replace '"sql":\s*null', '"sql": []' }
$linuxTarget = 'x86_64-unknown-linux-gnu'
$winBin = Join-Path $root 'target\release\rustcms.exe'
$linBin = Join-Path $root ('target\' + $linuxTarget + '\release\rustcms')

Write-Host ('== rustcms ' + $ver + ' ==')
Write-Host ('   root = ' + $root)
Write-Host ('   out  = ' + $out)

# ---------------------------------------------------------------- 编译
# Linux 交叉编译工具链：zig cc（下载解压到 E:\CodeBuddy\tools\zig-0.14.1\，
# zig.exe 自带 glibc 头与 crt，可同时充当 cc-rs 的 C 编译器与 cargo 的链接器）。
# 注意 cc-rs 会传 Rust 三元组 --target=x86_64-unknown-linux-gnu，zig 不认识，
# 所以统一走 zig-cc.exe 适配器（过滤调用方的 -target 参数、追加 zig 目标串）。
$zigDir = 'E:\CodeBuddy\tools\zig-0.14.1'
$zigExe = Join-Path $zigDir 'zig.exe'
$zigCC  = Join-Path $zigDir 'zig-cc.exe'
$zigAR  = Join-Path $zigDir 'zig-ar.exe'
$zigCCSrc = Join-Path $zigDir 'zig-cc.rs'
$zigARSrc = Join-Path $zigDir 'zig-ar.rs'
$rustc = Join-Path (Split-Path $cargo) 'rustc.exe'
foreach ($tool in @(@($zigCC, $zigCCSrc), @($zigAR, $zigARSrc))) {
  $exe = $tool[0]; $src = $tool[1]
  if ((Test-Path $zigExe) -and -not (Test-Path $exe)) {
    if (-not (Test-Path $src)) { throw ('缺 ' + $exe + ' 与源码 ' + $src + '：请按 deploy/upgrade.md 的 Linux 交叉编译说明准备 zig 工具链') }
    Push-Location $zigDir
    & $rustc -O $src -o $exe 2>$null
    Pop-Location
    if ($LASTEXITCODE -ne 0) { throw ($exe + ' 自动生成失败（rustc ' + $rustc + '）；请手工编译 ' + $src) }
  }
}
$linuxEnvSet = $false
if ((Test-Path $zigCC) -and (Test-Path $zigAR) -and -not $SkipLinux) {
  # cc-rs 调 <AR> cq lib.a ... 是 GNU ar 语法，必须走 zig-ar.exe（转成 zig ar cq ...）。
  $env:CC_x86_64_unknown_linux_gnu = $zigCC
  $env:CXX_x86_64_unknown_linux_gnu = $zigCC
  $env:AR_x86_64_unknown_linux_gnu = $zigAR
  $env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = $zigCC
  $linuxEnvSet = $true
}
# 机器提交内存紧张，交叉编译（全新依赖树）用单线程降低 rustc 峰值内存。
$env:CARGO_BUILD_JOBS = '1'
if (-not $SkipBuild) {
  Write-Host '== build =='
  Push-Location $root
  & $cargo build --release --offline -j 1
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw 'Windows 编译失败' }
  $linuxOk = $false
  if (-not $SkipLinux) {
    if ($linuxEnvSet) {
      & $cargo build --release --offline -j 1 --target $linuxTarget
      if ($LASTEXITCODE -ne 0) {
        Write-Host '  warning: Linux 交叉编译失败（zig cc 报错见上方），跳过 Linux 产物'
      } else { $linuxOk = $true }
    } else {
      & $cargo build --release --offline -j 1 --target $linuxTarget
      if ($LASTEXITCODE -ne 0) {
        Write-Host '  warning: Linux 交叉编译失败（缺 zig 工具链 / linker / target），跳过 Linux 产物'
      } else { $linuxOk = $true }
    }
  }
  Pop-Location
} else {
  $linuxOk = (-not $SkipLinux) -and (Test-Path $linBin)
}
if (-not (Test-Path $winBin)) { throw ('找不到 Windows 二进制：' + $winBin) }
if ($SkipLinux) { $linuxOk = $false }
if (-not $linuxOk -and -not $SkipLinux -and (Test-Path $linBin)) { $linuxOk = $true }
Write-Host ('  windows bin ' + (Get-Item $winBin).Length + ' bytes')
if ($linuxOk) { Write-Host ('  linux   bin ' + (Get-Item $linBin).Length + ' bytes') }

# ---------------------------------------------------------------- 归档工具
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function New-ZipFromDirectory {
  param([string]$Src, [string]$Out)
  if (Test-Path $Out) { Remove-Item -Force $Out }
  $base = (Resolve-Path $Src).Path.TrimEnd('\')
  $fs = [IO.File]::Open($Out, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
  $zip = New-Object IO.Compression.ZipArchive($fs, [IO.Compression.ZipArchiveMode]::Create)
  try {
    foreach ($f in (Get-ChildItem $Src -Recurse -File)) {
      $rel = $f.FullName.Substring($base.Length + 1).Replace('\', '/')
      $entry = $zip.CreateEntry($rel, [IO.Compression.CompressionLevel]::Optimal)
      $es = $entry.Open()
      $input = [IO.File]::OpenRead($f.FullName)
      try { $input.CopyTo($es) } finally { $input.Dispose(); $es.Dispose() }
    }
  } finally { $zip.Dispose(); $fs.Dispose() }
}

function Set-TarAscii {
  param([byte[]]$Buf, [int]$Offset, [int]$Length, [string]$Text)
  $bytes = [Text.Encoding]::ASCII.GetBytes($Text)
  $n = [Math]::Min($bytes.Length, $Length - 1)
  [Array]::Copy($bytes, 0, $Buf, $Offset, $n)
}
function Set-TarOctal {
  param([byte[]]$Buf, [int]$Offset, [int]$Length, [long]$Value)
  $s = [Convert]::ToString([long]$Value, 8).PadLeft($Length - 1, '0')
  if ($s.Length -gt ($Length - 1)) { $s = $s.Substring($s.Length - ($Length - 1)) }
  Set-TarAscii -Buf $Buf -Offset $Offset -Length $Length -Text $s
}
function New-TarHeader {
  param([string]$Name, [long]$Size, [int]$Mode, [long]$Mtime, [string]$Flag)
  $h = New-Object byte[] 512
  Set-TarAscii $h 0 100 $Name
  Set-TarOctal $h 100 8 $Mode
  Set-TarOctal $h 108 8 0
  Set-TarOctal $h 116 8 0
  Set-TarOctal $h 124 12 $Size
  Set-TarOctal $h 136 12 $Mtime
  for ($i = 148; $i -lt 156; $i++) { $h[$i] = 32 }
  if ($Flag.Length -gt 0) { $h[156] = [byte][char]$Flag.Substring(0, 1) }
  Set-TarAscii $h 257 6 'ustar'
  $h[263] = 48; $h[264] = 48
  $sum = 0L
  foreach ($b in $h) { $sum += [long]$b }
  Set-TarOctal $h 148 7 $sum
  $h[154] = 0
  $h[155] = 32
  return $h
}
function New-TarGzFromDirectory {
  param([string]$Src, [string]$Out, [string]$RootName)
  if (Test-Path $Out) { Remove-Item -Force $Out }
  $base = (Resolve-Path $Src).Path.TrimEnd('\')
  $fs = [IO.File]::Open($Out, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
  $gz = New-Object IO.Compression.GZipStream($fs, [IO.Compression.CompressionLevel]::Optimal)
  $epoch = [datetime]'1970-01-01T00:00:00Z'
  try {
    foreach ($f in (Get-ChildItem $Src -Recurse -File)) {
      $rel = $f.FullName.Substring($base.Length + 1).Replace('\', '/')
      $name = $RootName + '/' + $rel
      if ($name.Length -gt 99) { throw ('tar 路径过长：' + $name) }
      $lower = $rel.ToLowerInvariant()
      $exec = ($lower -eq 'rustcms') -or $lower.EndsWith('.sh') -or $lower.EndsWith('/rustcms')
      $mode = if ($exec) { 493 } else { 420 }   # 0755 / 0644
      $mtime = [long]([datetime]$f.LastWriteTimeUtc - $epoch).TotalSeconds
      $header = New-TarHeader -Name $name -Size $f.Length -Mode $mode -Mtime $mtime -Flag '0'
      $gz.Write($header, 0, 512)
      $data = [IO.File]::ReadAllBytes($f.FullName)
      $gz.Write($data, 0, $data.Length)
      $pad = (512 - ($data.Length % 512)) % 512
      if ($pad -gt 0) { $gz.Write((New-Object byte[] $pad), 0, $pad) }
    }
    $zeros = New-Object byte[] 1024
    $gz.Write($zeros, 0, $zeros.Length)
  } finally { $gz.Dispose(); $fs.Dispose() }
}

function Copy-RuntimeTree {
  param([string]$Dst, [string]$Bin, [string]$BinName, [string]$Arch)
  Remove-Item -Recurse -Force $Dst -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $Dst -Force | Out-Null
  Copy-Item $Bin (Join-Path $Dst $BinName) -Force
  Copy-Item -Recurse (Join-Path $root 'template') $Dst -Force
  Copy-Item -Recurse (Join-Path $root 'public')   $Dst -Force
  # 运行期生成的 sitemap 带本机域名 / 端口，打进包里会变成部署环境的死链，
  # 一律排除，让目标站首次访问 /sitemap.xml 时按自身配置重建。
  Get-ChildItem (Join-Path $Dst 'public') -Filter 'sitemap*.xml' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
  # 数据库安装包（结构 + 演示数据）；没有就跳过（纯升级包场景）
  $sqlSrc = Join-Path $root 'deploy\sql'
  if (Test-Path $sqlSrc) {
    $sqlDst = Join-Path $Dst 'deploy\sql'
    New-Item -ItemType Directory -Path $sqlDst -Force | Out-Null
    Copy-Item (Join-Path $sqlSrc '*.sql') $sqlDst -Force
  }
  foreach ($f in @('config.sample.json', 'README.md', 'LICENSE')) {
    Copy-Item (Join-Path $root $f) $Dst -Force
  }
  New-Item -ItemType Directory -Path (Join-Path $Dst 'logs')  -Force | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $Dst 'cache') -Force | Out-Null
  $nl = if ($BinName -like '*.exe') { "`r`n" } else { "`n" }
  [IO.File]::WriteAllText((Join-Path $Dst 'VERSION.txt'), ('rustcms ' + $ver + $nl), (New-Object Text.UTF8Encoding($false)))
  if ($Arch) {
    [IO.File]::WriteAllText((Join-Path $Dst 'PACKAGE_ARCH'), ($Arch + "`n"), (New-Object Text.UTF8Encoding($false)))
  }
  [IO.File]::WriteAllText((Join-Path $Dst 'version.json'), $vjSimpleTxt, (New-Object Text.UTF8Encoding($false)))
}

# ---------------------------------------------------------------- 安装包目录
Write-Host '== installer trees =='
$winDir = Join-Path $out ('rustcms-v' + $ver + '-windows')
Copy-RuntimeTree -Dst $winDir -Bin $winBin -BinName 'rustcms.exe' -Arch ''
$fixedWin = Join-Path $root 'deploy\windows'
if (Test-Path $fixedWin) {
  Get-ChildItem $fixedWin -File | ForEach-Object { Copy-Item $_.FullName $winDir -Force }
  # Windows 记事本 / PS 5.1 按 ANSI 读无 BOM 的 UTF-8 中文会乱码，说明文件要带 BOM。
  $readmeWin = Join-Path $winDir '使用说明.txt'
  if (Test-Path $readmeWin) {
    $readmeText = [IO.File]::ReadAllText($readmeWin, [Text.Encoding]::UTF8)
    [IO.File]::WriteAllText($readmeWin, $readmeText, (New-Object Text.UTF8Encoding($true)))
  }
}

$linDir = ''
if ($linuxOk) {
  $linDir = Join-Path $out ('rustcms-v' + $ver + '-linux-amd64')
  Copy-RuntimeTree -Dst $linDir -Bin $linBin -BinName 'rustcms' -Arch 'amd64'
  foreach ($f in @('install.sh', 'uninstall.sh')) {
    $src = Join-Path $root ('deploy\' + $f)
    if (Test-Path $src) { Copy-Item $src $linDir -Force }
  }
  $deployDst = Join-Path $linDir 'deploy'
  New-Item -ItemType Directory -Path $deployDst -Force | Out-Null
  foreach ($f in @('rustcms.service', 'upgrade.md')) {
    $src = Join-Path $root ('deploy\' + $f)
    if (Test-Path $src) { Copy-Item $src $deployDst -Force }
  }
}

# ---------------------------------------------------------------- 升级包
Write-Host '== upgrade packages =='
function New-UpgradeTree {
  param([string]$Dst, [string]$Bin, [string]$BinName)
  Remove-Item -Recurse -Force $Dst -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $Dst -Force | Out-Null
  Copy-Item $Bin (Join-Path $Dst $BinName) -Force
  Copy-Item -Recurse (Join-Path $root 'template') $Dst -Force
  Copy-Item -Recurse (Join-Path $root 'public')   $Dst -Force
  Get-ChildItem (Join-Path $Dst 'public') -Filter 'sitemap*.xml' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
  $sqlSrc = Join-Path $root 'deploy\sql'
  if (Test-Path $sqlSrc) {
    $sqlDst = Join-Path $Dst 'deploy\sql'
    New-Item -ItemType Directory -Path $sqlDst -Force | Out-Null
    Copy-Item (Join-Path $sqlSrc '*.sql') $sqlDst -Force
  }
  [IO.File]::WriteAllText((Join-Path $Dst 'version.json'), $vjSimpleTxt, (New-Object Text.UTF8Encoding($false)))
}

$upRoot = Join-Path $env:TEMP ('rustcms-up-' + $ver)
Remove-Item -Recurse -Force $upRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $upRoot -Force | Out-Null
$upWinDir = Join-Path $upRoot 'windows'
New-UpgradeTree -Dst $upWinDir -Bin $winBin -BinName 'rustcms.exe'

$upWinZip = Join-Path $out ('rustcms-v' + $ver + '-windows-upgrade.zip')
New-ZipFromDirectory -Src $upWinDir -Out $upWinZip
$upWinSum = (Get-FileHash $upWinZip -Algorithm SHA256).Hash.ToLower()

$upLinZip = ''
$upLinSum = ''
if ($linuxOk) {
  $upLinDir = Join-Path $upRoot 'linux'
  New-UpgradeTree -Dst $upLinDir -Bin $linBin -BinName 'rustcms'
  $upLinZip = Join-Path $out ('rustcms-v' + $ver + '-linux-upgrade.zip')
  New-ZipFromDirectory -Src $upLinDir -Out $upLinZip
  $upLinSum = (Get-FileHash $upLinZip -Algorithm SHA256).Hash.ToLower()
}
# 兼容旧面板的单包（内容 = Windows 升级包）
$upZip = Join-Path $out ('rustcms-v' + $ver + '.zip')
Copy-Item $upWinZip $upZip -Force
New-Item -ItemType Directory -Path $PSScriptRoot -Force | Out-Null
Copy-Item $upWinZip (Join-Path $PSScriptRoot ('rustcms-v' + $ver + '-windows-upgrade.zip')) -Force
Copy-Item $upWinZip (Join-Path $PSScriptRoot ('rustcms-v' + $ver + '.zip')) -Force
if ($linuxOk) {
  Copy-Item $upLinZip (Join-Path $PSScriptRoot ('rustcms-v' + $ver + '-linux-upgrade.zip')) -Force
}

# ---------------------------------------------------------------- 完整安装包归档
Write-Host '== full archives =='
$winFull = Join-Path $out ('rustcms-v' + $ver + '-windows.zip')
New-ZipFromDirectory -Src $winDir -Out $winFull
$winSum = (Get-FileHash $winFull -Algorithm SHA256).Hash.ToLower()

$linTgz = ''
$linSum = ''
if ($linuxOk) {
  $linTgz = Join-Path $out ('rustcms-v' + $ver + '-linux-amd64.tar.gz')
  New-TarGzFromDirectory -Src $linDir -Out $linTgz -RootName ('rustcms-v' + $ver + '-linux-amd64')
  $linSum = (Get-FileHash $linTgz -Algorithm SHA256).Hash.ToLower()
}

# ---------------------------------------------------------------- version.json
Write-Host '== version.json =='
$vj = [ordered]@{
  name     = 'rustcms'
  version  = $ver
  date     = (Get-Date -Format 'yyyy-MM-dd')
  url      = ('rustcms-v' + $ver + '.zip')
  checksum = ('sha256:' + $upWinSum)
  note     = $note
  sql      = $sqlList
  packages = [ordered]@{
    windows = [ordered]@{ url = ('rustcms-v' + $ver + '-windows-upgrade.zip'); checksum = ('sha256:' + $upWinSum) }
  }
  install  = [ordered]@{
    windows_file   = ('rustcms-v' + $ver + '-windows.zip')
    windows_sha256 = $winSum
  }
}
if ($linuxOk) {
  $vj['packages']['linux'] = [ordered]@{ url = ('rustcms-v' + $ver + '-linux-upgrade.zip'); checksum = ('sha256:' + $upLinSum) }
  $vj['install']['linux_amd64_file']   = ('rustcms-v' + $ver + '-linux-amd64.tar.gz')
  $vj['install']['linux_amd64_sha256'] = $linSum
}
$jtxt = ($vj | ConvertTo-Json -Depth 6)
if ($jtxt -match '"sql":\s*null') { $jtxt = $jtxt -replace '"sql":\s*null', '"sql": []' }
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'version.json'), $jtxt, (New-Object Text.UTF8Encoding($false)))
Copy-Item (Join-Path $PSScriptRoot 'version.json') (Join-Path $out 'version.json') -Force

# ---------------------------------------------------------------- 在线安装 / 卸载
$online = Join-Path $root 'deploy\online-install.sh'
if (Test-Path $online) {
  $txt = [IO.File]::ReadAllText($online).Replace('__DEFAULT_MIRROR__', $mirror)
  [IO.File]::WriteAllText((Join-Path $out 'install.sh'), $txt, (New-Object Text.UTF8Encoding($false)))
}
$uninst = Join-Path $root 'deploy\uninstall.sh'
if (Test-Path $uninst) {
  $txt = [IO.File]::ReadAllText($uninst)
  [IO.File]::WriteAllText((Join-Path $out 'uninstall.sh'), $txt, (New-Object Text.UTF8Encoding($false)))
}

# ---------------------------------------------------------------- 校验清单
Write-Host '== checksums =='
$pkgList = @($upZip, $upWinZip, $winFull)
if ($linuxOk) { $pkgList += @($upLinZip, $linTgz) }
$lines = @()
foreach ($f in $pkgList) {
  $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
  $sz = (Get-Item $f).Length
  $lines += ($h + '  ' + (Split-Path -Leaf $f) + '  ' + $sz)
  Write-Host ('  ' + (Split-Path -Leaf $f) + '  ' + [math]::Round($sz / 1MB, 2) + ' MB')
}
$hdr = @(
  ('# rustcms ' + $ver + ' release manifest'),
  ('# generated: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')),
  '# format: sha256  file  bytes',
  '#',
  ('# rustcms-v' + $ver + '-windows-upgrade.zip  upgrade package for Windows'),
  ('# rustcms-v' + $ver + '.zip                  legacy single upgrade package'),
  ('# rustcms-v' + $ver + '-windows.zip          full installer for Windows')
)
if ($linuxOk) {
  $hdr += @(
    ('# rustcms-v' + $ver + '-linux-upgrade.zip    upgrade package for Linux'),
    ('# rustcms-v' + $ver + '-linux-amd64.tar.gz   full installer for Linux x86_64')
  )
} else {
  $hdr += '# note: Linux packages were not built (no x86_64-linux-gnu cross linker on this host)'
}
$hdr += ''
[IO.File]::WriteAllLines((Join-Path $out 'SHA256SUMS.txt'), ($hdr + $lines), (New-Object Text.UTF8Encoding($false)))
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue

# ---------------------------------------------------------------- 版本号自增
# 每次正式发版（未指定 -SkipBuild）后把版本号 +1，下次打包自动使用新号。
# -SkipBuild 复用同一份二进制时不递增，避免包名与二进制内版本号不一致。
if (-not $SkipBuild -and $ver -match '^\d+\.\d+\.\d+$') {
  $parts = $ver.Split('.')
  $parts[$parts.Count - 1] = [string]([int]$parts[$parts.Count - 1] + 1)
  $next = $parts -join '.'
  $tomlPath = Join-Path $root 'Cargo.toml'
  $toml = [IO.File]::ReadAllText($tomlPath)
  $re = New-Object System.Text.RegularExpressions.Regex('(?m)^version\s*=\s*"[^"]+"')
  $toml = $re.Replace($toml, ('version = "' + $next + '"'), 1)
  [IO.File]::WriteAllText($tomlPath, $toml, (New-Object Text.UTF8Encoding($false)))
  $lockPath = Join-Path $root 'Cargo.lock'
  if (Test-Path $lockPath) {
    $lock = [IO.File]::ReadAllText($lockPath)
    $re2 = New-Object System.Text.RegularExpressions.Regex('(?s)(name = "rustcms"\r?\nversion = ")[^"]+(")')
    $lock = $re2.Replace($lock, ('${1}' + $next + '${2}'), 1)
    [IO.File]::WriteAllText($lockPath, $lock, (New-Object Text.UTF8Encoding($false)))
  }
  Write-Host ('next version -> ' + $next)
}

Write-Host 'done.'
