RELEASE_URL = https://github.com/dalance/veryl/releases/latest/download/

publish: publish_x86_64_linux publish_x86_64_windows publish_x86_64_mac publish_aarch64_mac

publish_x86_64_linux:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-linux.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	npm run deploy:linux-x64
	rm -rf bin

publish_x86_64_windows:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-windows.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl.exe veryl.zip
	npm run deploy:win32-x64
	rm -rf bin

publish_x86_64_mac:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-mac.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	npm run deploy:darwin-x64
	rm -rf bin

publish_aarch64_mac:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-aarch64-mac.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	npm run deploy:darwin-arm64
	rm -rf bin
