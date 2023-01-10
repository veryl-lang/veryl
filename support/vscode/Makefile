RELEASE_URL = https://github.com/dalance/veryl/releases/latest/download/

package: package_lnx package_win package_mac

package_lnx:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-linux.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	vsce package --target linux-x64
	rm -rf bin

package_win:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-windows.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl.exe veryl.zip
	vsce package --target win32-x64
	rm -rf bin

package_mac:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-mac.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	vsce package --target darwin-x64
	rm -rf bin
