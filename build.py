import zipfile
import os

if os.system("powershell ./build.ps1") != 0:
    print("编译失败")
    exit(1)

with zipfile.ZipFile("build.zip", "w") as zip:
    zip.write("./target/x86_64-pc-windows-gnu/release/sasa.dll", arcname="x86_64-pc-windows-gnu/sasa.dll")
    zip.write("./target/i686-pc-windows-gnu/release/sasa.dll", arcname="i686-pc-windows-gnu/sasa.dll")
    zip.write("./target/aarch64-linux-android/release/libsasa.so", arcname="aarch64-linux-android/libsasa.so")
    zip.write("./target/armv7-linux-androideabi/release/libsasa.so", arcname="armv7-linux-androideabi/libsasa.so")
    zip.write("./target/aarch64-apple-ios/release/libsasa.a", arcname="aarch64-apple-ios/libsasa.a")
