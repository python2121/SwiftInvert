# Qt shell over libSwiftInvertCore.so. Build (inside the swiftdev box):
#   swift build -c release --product SwiftInvertCore
#   cmake -S qt -B qt/build -G Ninja && cmake --build qt/build
#   qt/build/swiftinvert-qt [folder]
cmake_minimum_required(VERSION 3.22)
project(swiftinvert-qt CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

find_package(Qt6 REQUIRED COMPONENTS Widgets Concurrent DBus)

set(SWIFT_BUILD_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../.build/release"
    CACHE PATH "Directory containing libSwiftInvertCore.so")

add_executable(swiftinvert-qt main.cpp imageview.cpp levelswindow.cpp)
target_include_directories(swiftinvert-qt PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(swiftinvert-qt PRIVATE
    Qt6::Widgets Qt6::Concurrent Qt6::DBus
    ${SWIFT_BUILD_DIR}/libSwiftInvertCore.so)
# The dylib carries its own RUNPATH to the Swift runtime; the app only needs
# to find the dylib itself.
set_target_properties(swiftinvert-qt PROPERTIES
    BUILD_RPATH "${SWIFT_BUILD_DIR}")
