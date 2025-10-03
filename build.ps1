cargo build --release --target=i686-pc-windows-gnu --features "cpal"

cargo build --release --target=x86_64-pc-windows-gnu --features "cpal"


cargo build --target aarch64-linux-android --release --features "oboe"

cargo build --target armv7-linux-androideabi --release --features "oboe"

cargo build --release --target=aarch64-apple-ios --features "cpal"