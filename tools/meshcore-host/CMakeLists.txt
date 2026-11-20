cmake_minimum_required(VERSION 3.16)
project(meshcore_host CXX C)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# The official MeshCore sources are compiled unmodified. build.sh fetches them at
# the revision pinned in firmware/meshcore-gateway/MESHCORE_REVISION.
set(MESHCORE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/meshcore-src" CACHE PATH "MeshCore checkout")
set(MC ${MESHCORE_DIR})
set(CRYPTO ${CMAKE_CURRENT_SOURCE_DIR}/third_party/Crypto)

set(MESHCORE_SRC
  ${MC}/src/Dispatcher.cpp
  ${MC}/src/Mesh.cpp
  ${MC}/src/Packet.cpp
  ${MC}/src/Identity.cpp
  ${MC}/src/Utils.cpp
  ${MC}/src/helpers/BaseChatMesh.cpp
  ${MC}/src/helpers/StaticPoolPacketManager.cpp
  ${MC}/src/helpers/AdvertDataHelpers.cpp
  ${MC}/src/helpers/TxtDataHelpers.cpp
)
file(GLOB ED25519_SRC ${MC}/lib/ed25519/*.c)
set(CRYPTO_SRC
  ${CRYPTO}/AES128.cpp ${CRYPTO}/AESCommon.cpp ${CRYPTO}/BlockCipher.cpp ${CRYPTO}/Crypto.cpp
  ${CRYPTO}/Hash.cpp ${CRYPTO}/SHA256.cpp ${CRYPTO}/SHA512.cpp ${CRYPTO}/Ed25519.cpp
  ${CRYPTO}/Curve25519.cpp ${CRYPTO}/BigNumberUtil.cpp
)

add_executable(meshcore-host
  app/main.cpp app/KissHub.cpp app/VirtualNode.cpp app/crypto_rng_stub.cpp
  ${MESHCORE_SRC} ${ED25519_SRC} ${CRYPTO_SRC})

target_include_directories(meshcore-host PRIVATE
  shim third_party/base64/src ${MC}/src ${MC}/lib/ed25519 ${CRYPTO} app)

# must be identical in every translation unit (affects class layout)
target_compile_definitions(meshcore-host PRIVATE MAX_CONTACTS=200 MAX_GROUP_CHANNELS=8)

if(WIN32)
  target_compile_definitions(meshcore-host PRIVATE _CRT_SECURE_NO_WARNINGS NOMINMAX WIN32_LEAN_AND_MEAN)
  target_link_libraries(meshcore-host PRIVATE bcrypt ws2_32)
  if(MSVC)
    target_compile_options(meshcore-host PRIVATE /utf-8 /W1 -Wno-everything)
  endif()
else()
  find_package(Threads REQUIRED)
  target_link_libraries(meshcore-host PRIVATE Threads::Threads)
  # The MeshCore and Crypto sources are third-party code: no warnings from them.
  target_compile_options(meshcore-host PRIVATE -w)
endif()
