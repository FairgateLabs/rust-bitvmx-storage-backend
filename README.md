# rust-bitvmx-storage-backend
A Rust library for managing storage for BitVMX 

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