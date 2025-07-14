# Installation Guide

This guide explains how to install **RocksDB, Clang (LLVM), and compression libraries** for Rust development across **macOS, Linux, and Windows** to avoid triggering the RocksDB custom build that rebuilds the RocksDB library with every change. It also includes **environment variable setup** and **VS Code integration** to ensure `rust-analyzer` works correctly.

## Prerequisites

The project relies on Clang for compiling certain C/C++ code. Install Clang on your system:

- **Windows**: Download and install [LLVM](https://llvm.org/releases/download.html) or use **MSYS2**:
  ```bash
  pacman -S mingw-w64-x86_64-clang
  ```
- **Linux**: Install Clang via your package manager:
    ```bash
  sudo apt install clang
  ```

- **macOS**: Clang comes pre-installed with Xcode or can be installed via Homebrew:
  ```bash
  brew install llvm
  ```

Ensure the following environment variables are set:
```bash
# On Windows
set CC=clang
set CXX=clang++
set AR=llvm-ar

# On Unix-like systems
export CC=clang
export CXX=clang++
export AR=llvm-ar
```

---

## **1. Install Required Libraries**
You need:
- **RocksDB** (for database storage)
- **LLVM & Clang** (for Rust bindings)
- **Compression Libraries** (Bzip2, LZ4, ZSTD, Snappy, Zlib)

### **macOS (Apple Silicon & Intel)**
```sh
brew install rocksdb llvm bzip2 lz4 zstd snappy zlib
```

### **Linux (Debian/Ubuntu)**
```sh
sudo apt update
sudo apt install -y librocksdb-dev llvm-dev libclang-dev bzip2 liblz4-dev libzstd-dev libsnappy-dev zlib1g-dev
```

### **Windows (Using vcpkg)**
```sh
vcpkg install rocksdb:x64-windows
vcpkg install llvm:x64-windows
vcpkg install bzip2 lz4 zstd snappy zlib:x64-windows
```

---

## **2. Set Environment Variables**
### **macOS**
```sh
export LIBCLANG_PATH="/opt/homebrew/opt/llvm/lib"
export DYLD_LIBRARY_PATH="/opt/homebrew/opt/llvm/lib"
export ROCKSDB_LIB_DIR="/opt/homebrew/lib"
export ROCKSDB_INCLUDE_DIR="/opt/homebrew/include"
```
Make it permanent:
```sh
echo 'export LIBCLANG_PATH="/opt/homebrew/opt/llvm/lib"' >> ~/.zshrc
echo 'export DYLD_LIBRARY_PATH="/opt/homebrew/opt/llvm/lib"' >> ~/.zshrc
echo 'export ROCKSDB_LIB_DIR="/opt/homebrew/lib"' >> ~/.zshrc
echo 'export ROCKSDB_INCLUDE_DIR="/opt/homebrew/include"' >> ~/.zshrc
source ~/.zshrc
```

### **Linux**
```sh
export LIBCLANG_PATH="/usr/lib/llvm-12/lib"
export LD_LIBRARY_PATH="/usr/lib/llvm-12/lib"
export ROCKSDB_LIB_DIR="/usr/lib"
export ROCKSDB_INCLUDE_DIR="/usr/include"
```
Make it permanent:
```sh
echo 'export LIBCLANG_PATH="/usr/lib/llvm-12/lib"' >> ~/.bashrc
echo 'export LD_LIBRARY_PATH="/usr/lib/llvm-12/lib"' >> ~/.bashrc
echo 'export ROCKSDB_LIB_DIR="/usr/lib"' >> ~/.bashrc
echo 'export ROCKSDB_INCLUDE_DIR="/usr/include"' >> ~/.bashrc
source ~/.bashrc
```

### **Windows (PowerShell)**
```powershell
$env:LIBCLANG_PATH="C:\vcpkg\installed\x64-windows\lib"
$env:ROCKSDB_LIB_DIR="C:\vcpkg\installed\x64-windows\lib"
$env:ROCKSDB_INCLUDE_DIR="C:\vcpkg\installed\x64-windows\include"
```
Make it permanent:
```powershell
[System.Environment]::SetEnvironmentVariable("LIBCLANG_PATH", "C:\\vcpkg\\installed\\x64-windows\\lib", "User")
[System.Environment]::SetEnvironmentVariable("ROCKSDB_LIB_DIR", "C:\\vcpkg\\installed\\x64-windows\\lib", "User")
[System.Environment]::SetEnvironmentVariable("ROCKSDB_INCLUDE_DIR", "C:\\vcpkg\\installed\\x64-windows\\include", "User")
```
Then restart your terminal.

---

## **3. Optional Environment Variables**
These variables are not required for Rust development but may be needed in certain cases.

### **macOS**
To make these changes permanent, add them to `~/.zshrc`:
```sh
# clang or other LLVM tools
echo 'export PATH="/opt/homebrew/opt/llvm/bin:$PATH"' >> ~/.zshrc

# LDFLAGS and CPPFLAGS for compilation
echo 'export LDFLAGS="-L/opt/homebrew/opt/llvm/lib"' >> ~/.zshrc
echo 'export CPPFLAGS="-I/opt/homebrew/opt/llvm/include"' >> ~/.zshrc

# Bzip2 paths
echo 'export PATH="/opt/homebrew/opt/bzip2/bin:$PATH"' >> ~/.zshrc
echo 'export LDFLAGS="-L/opt/homebrew/opt/bzip2/lib"' >> ~/.zshrc
source ~/.zshrc
```

### **Linux**
For Linux, add them to `~/.bashrc`:
```sh
# clang or other LLVM tools
echo 'export PATH="/usr/lib/llvm-12/bin:$PATH"' >> ~/.bashrc
echo 'export LDFLAGS="-L/usr/lib/llvm-12/lib"' >> ~/.bashrc

# LDFLAGS and CPPFLAGS for compilation
echo 'export CPPFLAGS="-I/usr/include/llvm-12"' >> ~/.bashrc
echo 'export PATH="/usr/local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### **Windows (PowerShell)**
For Windows, set these variables permanently:
```powershell
[System.Environment]::SetEnvironmentVariable("PATH", "C:\\vcpkg\\installed\\x64-windows\\bin;" + $env:PATH, "User")
[System.Environment]::SetEnvironmentVariable("LDFLAGS", "-L C:\\vcpkg\\installed\\x64-windows\\lib", "User")
[System.Environment]::SetEnvironmentVariable("CPPFLAGS", "-I C:\\vcpkg\\installed\\x64-windows\\include", "User")
```
Then restart your terminal.

---

## **4. Ensure VS Code Uses These Variables**
VS Code does **not** inherit terminal environment variables by default.

### **Option 1: Open VS Code from Terminal**
```sh
code .
```
This **inherits your environment**.

### **Option 2: Set `rust-analyzer` Environment in VS Code**
1. **Open VS Code**  
2. **Go to** `Settings` (Cmd + ,)  
3. **Search for** `rust-analyzer.server.extraEnv`  
4. **Click "Edit in settings.json"**  
5. **Add:**
```json
"rust-analyzer.server.extraEnv": {
    "LIBCLANG_PATH": "/opt/homebrew/opt/llvm/lib",
    "DYLD_LIBRARY_PATH": "/opt/homebrew/opt/llvm/lib",
    "ROCKSDB_LIB_DIR": "/opt/homebrew/lib",
    "ROCKSDB_INCLUDE_DIR": "/opt/homebrew/include"
}
```
6. **Restart VS Code**  

### **Option 3: Use `launchctl` for macOS (System-Wide Fix)**
```sh
launchctl setenv LIBCLANG_PATH /opt/homebrew/opt/llvm/lib
launchctl setenv DYLD_LIBRARY_PATH /opt/homebrew/opt/llvm/lib
launchctl setenv ROCKSDB_LIB_DIR /opt/homebrew/lib
launchctl setenv ROCKSDB_INCLUDE_DIR /opt/homebrew/include
```
Then restart VS Code.

---

## **5. Debugging Common Issues**
### **Problem: `libclang.dylib` Not Found in VS Code**
**Fix:**  
- Open VS Code from the terminal (`code .`)
- Add `rust-analyzer.server.extraEnv` in VS Code settings.
- Use `launchctl` on macOS to set env vars system-wide.

### **Problem: `dyld: Library not loaded: @rpath/libclang.dylib`**
**Fix:**  
```sh
sudo install_name_tool -add_rpath /opt/homebrew/opt/llvm/lib $(which rustc)
sudo install_name_tool -add_rpath /opt/homebrew/opt/llvm/lib $(which cargo)
```
Then check:
```sh
otool -L $(which rustc)
otool -L $(which cargo)
```

### **Problem: RocksDB Compilation Fails**
**Fix:**  
- Ensure `ROCKSDB_LIB_DIR` and `ROCKSDB_INCLUDE_DIR` are set correctly.
- Manually link missing compression libraries (`bzip2, lz4, zstd, snappy, zlib`). 