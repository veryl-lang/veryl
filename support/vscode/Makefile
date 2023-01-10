RELEASE_URL = https://github.com/dalance/veryl/releases/latest/download/

publish: publish_lnx publish_win publish_mac

publish_lnx:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-linux.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	npm run deploy:linux-x64
	rm -rf bin

publish_win:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-windows.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl.exe veryl.zip
	npm run deploy:win32-x64
	rm -rf bin

publish_mac:
	mkdir -p bin
	curl -L -o veryl.zip ${RELEASE_URL}/veryl-x86_64-mac.zip
	unzip -o -d bin veryl.zip
	rm bin/veryl veryl.zip
	npm run deploy:darwin-x64
	rm -rf bin
