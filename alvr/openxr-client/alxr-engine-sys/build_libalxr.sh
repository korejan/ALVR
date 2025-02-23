cd cpp/ALVR-OpenXR-Engine
rm -rf build
cmake -GNinja -DCMAKE_BUILD_TYPE=RelWithDebInfo \
 -DDYNAMIC_LOADER:BOOL=OFF \
 -DBUILD_WITH_SYSTEM_JSONCPP:BOOL=OFF \
 -DBUILD_CUDA_INTEROP:BOOL=OFF \
 -DDISABLE_DECODER_SUPPORT:BOOL=ON \
 -DCMAKE_INSTALL_PREFIX='../../../../../build/libalxr' \
 -B build
ninja install -C build
rm -rf build
