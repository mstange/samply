export SDKROOT=/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
MACOSX_DEPLOYMENT_TARGET=10.12 cargo build --release --target=x86_64-apple-darwin
mv target/x86_64-apple-darwin/release/libsamply_mac_preload.dylib binaries/libsamply_mac_preload_x86_64.dylib
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target=aarch64-apple-darwin
mv target/aarch64-apple-darwin/release/libsamply_mac_preload.dylib binaries/libsamply_mac_preload_arm64.dylib
MACOSX_DEPLOYMENT_TARGET=11.0 RUSTC_BOOTSTRAP=1 cargo build --release --target=arm64e-apple-darwin -Zbuild-std
mv target/arm64e-apple-darwin/release/libsamply_mac_preload.dylib binaries/libsamply_mac_preload_arm64e.dylib
lipo binaries/libsamply_mac_preload_* -create -output binaries/libsamply_mac_preload.dylib
gzip -cvf binaries/libsamply_mac_preload.dylib > ../samply/resources/libsamply_mac_preload.dylib.gz
