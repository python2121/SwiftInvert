# Command Line Tools ship Testing.framework outside the default search paths
# (and its lib_TestingInterop.dylib in a second directory), so tests need
# explicit framework + rpath flags. `swift build` / `swift run` need nothing.
CLT_FW := /Library/Developer/CommandLineTools/Library/Developer/Frameworks
CLT_LIB := /Library/Developer/CommandLineTools/Library/Developer/usr/lib
TEST_FLAGS := -Xswiftc -F$(CLT_FW) -Xlinker -F$(CLT_FW) \
	-Xlinker -rpath -Xlinker $(CLT_FW) -Xlinker -rpath -Xlinker $(CLT_LIB)

.PHONY: build test run release

build:
	swift build

test:
	swift test $(TEST_FLAGS)

run:
	swift run SwiftInvert

release:
	swift build -c release
