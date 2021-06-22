To cross-compile for Windows.
```
rustup target add x86_64-pc-windows-gnu
rustup toolchain install nightly-x86_64-pc-windows-gnu
brew install mingw-w64
cargo build --target x86_64-pc-windows-gnu --release
```

To look at the help.
`cargo run --bin proton -- -h`