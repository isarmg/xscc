# ventoy-img 技术文档

## 概述

ventoy-img 是一个纯 Rust 实现的 Ventoy 可启动镜像生成工具，无需 root 权限或 loop 设备即可创建完整可用的 Ventoy IMG 文件。

## 架构设计

```
┌─────────────────────────────────────────────────────────────┐
│                        CLI (main.rs)                        │
├─────────────────────────────────────────────────────────────┤
│                    VentoyImage (image.rs)                   │
├──────────────┬──────────────┬──────────────┬───────────────┤
│   Partition  │    exFAT     │   Resources  │     Error     │
│ (partition.rs)│  (exfat/)   │(resources.rs)│  (error.rs)   │
└──────────────┴──────────────┴──────────────┴───────────────┘
```

## 核心模块

### 1. 分区模块 (partition.rs)

负责 MBR 分区表的创建和管理。

#### 分区布局

```
┌────────────────────────────────────────────────────────────┐
│ Sector 0: MBR (Boot Code + Partition Table + Signature)    │
├────────────────────────────────────────────────────────────┤
│ Sector 1-2047: GRUB core.img (引导代码)                    │
├────────────────────────────────────────────────────────────┤
│ Sector 2048 - N: 数据分区 (exFAT, 存放 ISO 文件)           │
├────────────────────────────────────────────────────────────┤
│ Sector N+1 - End: EFI 分区 (FAT16, 32MB, UEFI 启动)        │
└────────────────────────────────────────────────────────────┘
```

#### MBR 结构 (512 字节)

| 偏移 | 大小 | 内容 |
|------|------|------|
| 0x000 | 440 | Boot Code (来自 boot.img) |
| 0x1B8 | 4 | Disk Signature |
| 0x1BC | 2 | Reserved |
| 0x1BE | 16 | Partition Entry 1 (数据分区) |
| 0x1CE | 16 | Partition Entry 2 (EFI 分区) |
| 0x1DE | 16 | Partition Entry 3 (未使用) |
| 0x1EE | 16 | Partition Entry 4 (未使用) |
| 0x1FE | 2 | Boot Signature (0x55AA) |

#### Ventoy 签名

位于 MBR 偏移 0x190，16 字节：
```
56 54 00 47 65 00 48 44 00 52 64 00 20 45 72 0D
```

### 2. exFAT 模块 (exfat/)

完整的 exFAT 文件系统实现，支持读写操作。

#### 2.1 格式化 (format.rs)

创建 exFAT 文件系统结构：

```
┌─────────────────────────────────────────────────────────────┐
│ Boot Region (Sector 0-11)                                   │
│   - Boot Sector (512 bytes)                                 │
│   - Extended Boot Sectors (8 sectors)                       │
│   - OEM Parameters (2 sectors)                              │
│   - Boot Checksum Sector                                    │
├─────────────────────────────────────────────────────────────┤
│ Backup Boot Region (Sector 12-23)                           │
├─────────────────────────────────────────────────────────────┤
│ FAT Region (Sector 24+)                                     │
│   - FAT Table (每个 cluster 4 字节)                         │
├─────────────────────────────────────────────────────────────┤
│ Cluster Heap                                                │
│   - Cluster 2: Allocation Bitmap                            │
│   - Cluster 3..N: Upcase Table (128KB，可能跨多个簇)        │
│   - Cluster N+1: Root Directory                             │
│   - Cluster N+2+: 用户数据                                  │
└─────────────────────────────────────────────────────────────┘
```

##### 动态簇大小

根据卷大小自动选择最优簇大小：

| 卷大小 | 簇大小 | 说明 |
|--------|--------|------|
| < 256MB | 4KB | 适合小文件，减少浪费 |
| 256MB - 8GB | 32KB | 平衡性能和空间 |
| > 8GB | 128KB | 优化大文件 (ISO) 性能 |

```rust
fn get_cluster_size(total_sectors: u64) -> u32 {
    let volume_size = total_sectors * 512;
    match volume_size {
        n if n < 256 * 1024 * 1024 => 4096,        // < 256MB
        n if n < 8 * 1024 * 1024 * 1024 => 32768,  // 256MB - 8GB
        _ => 128 * 1024,                            // > 8GB
    }
}
```

#### Boot Sector 关键字段

| 偏移 | 大小 | 字段 | 说明 |
|------|------|------|------|
| 0 | 3 | JumpBoot | 跳转指令 (0xEB 0x76 0x90) |
| 3 | 8 | FileSystemName | "EXFAT   " |
| 64 | 8 | PartitionOffset | 分区偏移 |
| 72 | 8 | VolumeLength | 卷大小（扇区数） |
| 80 | 4 | FatOffset | FAT 起始扇区 |
| 84 | 4 | FatLength | FAT 长度（扇区数） |
| 88 | 4 | ClusterHeapOffset | Cluster Heap 起始扇区 |
| 92 | 4 | ClusterCount | Cluster 总数 |
| 96 | 4 | FirstClusterOfRootDirectory | 根目录起始 Cluster |
| 100 | 4 | VolumeSerialNumber | 卷序列号 |
| 108 | 1 | BytesPerSectorShift | 扇区大小位移 (9 = 512) |
| 109 | 1 | SectorsPerClusterShift | Cluster 大小位移 |
| 510 | 2 | BootSignature | 0xAA55 |

#### 2.2 文件操作 (ops.rs)

##### Cluster 编号规则

- Cluster 0, 1: 保留
- Cluster 2: Allocation Bitmap
- Cluster 3: Upcase Table
- Cluster 4: Root Directory
- Cluster 5+: 用户数据

##### FAT 表条目值

| 值 | 含义 |
|----|------|
| 0x00000000 | 空闲 |
| 0x00000002 - 0xFFFFFFF6 | 下一个 Cluster |
| 0xFFFFFFF7 | 坏 Cluster |
| 0xFFFFFFF8 - 0xFFFFFFFF | 链结束 |

##### 目录条目类型

| 类型 | 值 | 说明 |
|------|-----|------|
| Volume Label | 0x83 | 卷标 |
| Allocation Bitmap | 0x81 | 位图描述 |
| Upcase Table | 0x82 | 大写表描述 |
| File | 0x85 | 文件/目录 |
| Stream Extension | 0xC0 | 流扩展 |
| File Name | 0xC1 | 文件名 |

##### 文件条目集结构

创建一个文件需要 3+ 个目录条目：

```
┌─────────────────────────────────────────────────────────────┐
│ File Directory Entry (0x85) - 32 bytes                      │
│   - EntryType: 0x85                                         │
│   - SecondaryCount: 后续条目数                              │
│   - SetChecksum: 校验和                                     │
│   - FileAttributes: 属性                                    │
│   - Timestamps: 创建/修改/访问时间                          │
├─────────────────────────────────────────────────────────────┤
│ Stream Extension Entry (0xC0) - 32 bytes                    │
│   - EntryType: 0xC0                                         │
│   - GeneralSecondaryFlags: 标志                             │
│   - NameLength: 文件名长度 (UTF-16 字符数)                  │
│   - NameHash: 文件名哈希                                    │
│   - FirstCluster: 数据起始 Cluster                          │
│   - DataLength: 文件大小                                    │
├─────────────────────────────────────────────────────────────┤
│ File Name Entry (0xC1) - 32 bytes × N                       │
│   - EntryType: 0xC1                                         │
│   - FileName: 15 个 UTF-16 字符                             │
└─────────────────────────────────────────────────────────────┘
```

##### 校验和算法

```rust
// Entry Set Checksum
fn calculate_entry_set_checksum(entries: &[[u8; 32]]) -> u16 {
    let mut checksum: u16 = 0;
    for (entry_idx, entry) in entries.iter().enumerate() {
        for (byte_idx, &byte) in entry.iter().enumerate() {
            // 跳过第一个条目的校验和字段 (bytes 2-3)
            if entry_idx == 0 && (byte_idx == 2 || byte_idx == 3) {
                continue;
            }
            checksum = checksum.rotate_right(1).wrapping_add(byte as u16);
        }
    }
    checksum
}

// Name Hash (使用 Unicode 大写转换)
fn calculate_name_hash(name: &str) -> u16 {
    let mut hash: u16 = 0;
    for ch in name.encode_utf16() {
        let upper = unicode::to_uppercase_simple(ch); // 使用 Unicode 模块
        let bytes = upper.to_le_bytes();
        hash = hash.rotate_right(1).wrapping_add(bytes[0] as u16);
        hash = hash.rotate_right(1).wrapping_add(bytes[1] as u16);
    }
    hash
}
```

#### 2.3 Unicode 模块 (unicode.rs)

提供 exFAT 文件名的 Unicode 支持：

##### UTF-16 大写转换

支持以下字符范围的大小写转换：
- ASCII (a-z)
- Latin-1 Supplement (à-ÿ)
- Latin Extended-A (ā-ž)
- Greek (α-ω)
- Cyrillic (а-я, ѐ-џ)

```rust
pub fn to_uppercase_simple(ch: u16) -> u16 {
    match ch {
        0x0061..=0x007A => ch - 32,                    // ASCII a-z
        0x00E0..=0x00F6 | 0x00F8..=0x00FE => ch - 32,  // Latin-1
        0x03B1..=0x03C1 => ch - 32,                    // Greek α-ρ
        0x03C3..=0x03C9 => ch - 32,                    // Greek σ-ω
        0x03C2 => 0x03A3,                               // ς -> Σ
        0x0430..=0x044F => ch - 32,                    // Cyrillic а-я
        0x0450..=0x045F => ch - 80,                    // Cyrillic ѐ-џ
        // ... 更多 Latin Extended-A 映射
        _ => ch,
    }
}
```

##### Upcase Table

生成 128KB 的 Upcase 表，映射每个 UTF-16 代码单元到其大写形式：

```rust
pub fn generate_upcase_table() -> Vec<u8> {
    let mut table = Vec::with_capacity(65536 * 2);
    for i in 0u32..65536 {
        let upper = to_uppercase_simple(i as u16);
        table.extend_from_slice(&upper.to_le_bytes());
    }
    table  // 128KB
}
```

##### UTF-16 编解码

支持 BMP 和补充平面字符（如 Emoji）：

```rust
// 编码
pub fn encode_utf16le(s: &str) -> Vec<u8>

// 解码（处理代理对）
pub fn decode_utf16le(bytes: &[u8]) -> String
```

### 3. 资源模块 (resources.rs)

内嵌 Ventoy 启动所需的二进制资源：

| 资源 | 大小 | 用途 |
|------|------|------|
| boot.img | 512 bytes | MBR 引导代码 |
| core.img.xz | ~448 KB | GRUB 核心镜像 (XZ 压缩) |
| ventoy.disk.img.xz | ~13 MB | EFI 分区镜像 (XZ 压缩) |

资源使用 `include_bytes!` 宏在编译时嵌入，运行时使用 `lzma-rs` 解压。

### 4. 错误处理 (error.rs)

使用 `thiserror` 定义错误类型：

```rust
#[derive(Debug, thiserror::Error)]
pub enum VentoyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid size format: {0}")]
    InvalidSize(String),

    #[error("Image error: {0}")]
    ImageError(String),

    #[error("Filesystem error: {0}")]
    FilesystemError(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Decompression error: {0}")]
    DecompressionError(String),
}
```

## 关键实现细节

### Cluster 大小选择

根据卷大小动态选择 cluster 大小：
- < 256MB: 4KB clusters (适合小文件)
- 256MB - 8GB: 32KB clusters (平衡)
- > 8GB: 128KB clusters (优化大文件性能)

Upcase Table 固定为 128KB (65536 × 2 bytes)，可能跨多个 cluster。

### 流式读写

#### ExfatFileWriter

支持流式写入大文件：

```rust
pub struct ExfatFileWriter<'a> {
    fs: &'a mut ExfatFs,
    name: String,
    total_size: u64,           // 必须预先知道
    allocated_clusters: Vec<u32>,
    current_cluster_index: usize,
    cluster_buffer: Vec<u8>,   // 缓冲区（簇大小）
    bytes_written: u64,
}
```

写入流程：
1. 预先分配所有需要的 clusters
2. 数据写入 cluster_buffer
3. 缓冲区满时写入当前 cluster
4. finish() 时创建目录条目

#### ExfatFileReader

支持流式读取大文件，实现 `std::io::Read` 和 `std::io::Seek`：

```rust
pub struct ExfatFileReader<'a> {
    fs: &'a mut ExfatFs,
    cluster_chain: Vec<u32>,   // 文件的 cluster 链
    file_size: u64,            // 文件总大小
    position: u64,             // 当前读取位置
    cluster_cache: Option<(u32, Vec<u8>)>,  // 当前 cluster 缓存
}
```

读取流程：
1. 根据 position 计算当前 cluster 索引和偏移
2. 如果 cluster 不在缓存中，读取并缓存
3. 从缓存中复制数据到用户缓冲区
4. 更新 position

Seek 支持：
- `SeekFrom::Start(n)` - 从文件开头偏移
- `SeekFrom::Current(n)` - 从当前位置偏移
- `SeekFrom::End(n)` - 从文件结尾偏移

### 文件名大小写

exFAT 文件名大小写不敏感但保留大小写：
- 查找时转换为小写比较
- 存储时保留原始大小写
- Name Hash 使用大写计算

## 性能考虑

1. **稀疏文件**: 使用 `file.set_len()` 创建稀疏文件，避免写入全零
2. **批量写入**: cluster 级别批量写入，减少 I/O 次数
3. **内存映射**: 未使用 mmap，保持跨平台兼容性
4. **缓冲**: 流式写入使用 128KB 缓冲区

## 限制

1. ~~仅支持根目录文件操作~~ ✅ 已支持子目录
2. ~~不支持文件覆盖~~ ✅ 已支持文件覆盖
3. ~~目录条目限制在单个 cluster 内~~ ✅ 已支持目录扩展（多 cluster）
4. ~~仅支持 ASCII 文件名~~ ✅ 已支持完整 Unicode（中日韩、西里尔、希腊、Emoji）
5. 不支持扩展属性和 ACL

## 新增功能

### 子目录支持

支持完整的目录操作：
- 路径解析：支持 `path/to/file` 格式
- 创建目录：支持递归创建父目录（mkdir -p）
- 目录遍历：支持遍历多 cluster 的目录
- 递归列出：支持列出所有子目录中的文件
- 递归删除：支持删除目录及其所有内容

```rust
// 路径解析
fn parse_path(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

// 解析路径到目标目录
fn resolve_path(&mut self, path: &str, create_parents: bool) -> Result<ResolvedPath>
```

### 文件覆盖支持

所有写入方法都支持覆盖选项：
- `write_file_overwrite()` - 覆盖根目录文件
- `write_file_path()` - 支持 `overwrite` 参数
- `ExfatFileWriter::create_overwrite()` - 流式写入覆盖
- `ExfatFileWriter::create_at_path()` - 指定路径 + 覆盖

覆盖逻辑：
1. 检查目标文件是否存在
2. 如果存在且 `overwrite=true`，先删除旧文件
3. 创建新文件

### 目录扩展支持

当目录中的文件数量超过单个 cluster 容量时，自动扩展目录：

```rust
fn find_free_slot_in_directory(&mut self, dir_cluster: u32, entries_needed: usize) -> Result<(u32, u32)> {
    // 1. 遍历目录链中的所有 cluster
    // 2. 查找连续的空闲条目
    // 3. 如果空间不足，调用 extend_cluster_chain() 分配新 cluster
    // 4. 清除旧 cluster 中的 END 标记
    // 5. 返回新 cluster 和偏移
}

fn extend_cluster_chain(&mut self, first_cluster: u32) -> Result<u32> {
    // 1. 读取 cluster 链，找到最后一个 cluster
    // 2. 分配一个新 cluster
    // 3. 更新 FAT 表链接
    // 4. 初始化新 cluster 为零
    // 5. 返回新 cluster 编号
}
```

### 流式读取支持

`ExfatFileReader` 支持流式读取大文件：

```rust
use bootable_virtual_media::exfat::ExfatFileReader;
use std::io::{Read, Seek, SeekFrom};

// 打开文件
let mut reader = ExfatFileReader::open(&mut fs, "large.iso")?;

// 获取文件信息
println!("Size: {}, Position: {}", reader.file_size(), reader.position());

// 读取数据
let mut buf = vec![0u8; 4096];
let n = reader.read(&mut buf)?;

// Seek 操作
reader.seek(SeekFrom::Start(1024))?;
reader.seek(SeekFrom::Current(100))?;
reader.seek(SeekFrom::End(-100))?;
```

特性：
- 实现 `std::io::Read` 和 `std::io::Seek` 特征
- Cluster 级别缓存，减少 I/O
- 支持任意位置 seek
- 内存占用低（只缓存当前 cluster）

### Unicode 支持

完整的 Unicode 文件名支持：

支持的字符范围：
- ASCII (a-z, A-Z)
- Latin-1 Supplement (à-ÿ, À-Þ)
- Latin Extended-A (ā-ž)
- Greek (α-ω, Α-Ω)
- Cyrillic (а-я, А-Я, ѐ-џ, Ѐ-Џ)
- CJK 字符（中日韩）
- Emoji（通过 UTF-16 代理对）

```rust
// 支持 Unicode 文件名
fs.write_file("中文文件.txt", b"content")?;
fs.write_file("Файл.txt", b"content")?;     // 俄语
fs.write_file("αβγ.txt", b"content")?;       // 希腊语
fs.write_file("😀🎉.txt", b"content")?;       // Emoji

// 大小写不敏感查找
let data = fs.read_file("ФАЙЛ.TXT")?;        // 找到 Файл.txt
```

## 参考资料

- [exFAT File System Specification](https://docs.microsoft.com/en-us/windows/win32/fileio/exfat-specification)
- [Ventoy Official Documentation](https://www.ventoy.net/en/doc_start.html)
- [GRUB Manual](https://www.gnu.org/software/grub/manual/grub/)
