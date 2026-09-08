# sysview — Linux 系統觀測工具
#
# 一般使用：  make && sudo make install
# 只編譯：    make
# 只安裝：    sudo make install
#
# 安裝位置：
#   $(PREFIX)/bin/sysview                       一般使用者執行，不需要 root
#   $(PREFIX)/libexec/sysview/sysview-priv      root:root 0755，**絕不 setuid**
#
# 安裝器不會、也不該修改你的 sudo 政策。見 docs/sudo.md。

PREFIX      ?= /usr/local
# 彩蛋排行榜的共用目錄（見 docs/dino.md）
SCOREDIR   ?= /var/lib/sysview/dino
BINDIR      ?= $(PREFIX)/bin
LIBEXECDIR  ?= $(PREFIX)/libexec/sysview
DOCDIR      ?= $(PREFIX)/share/doc/sysview
# 原始碼的共用位置。FHS 規定 /usr/local/src 就是放本機自行編譯的原始碼。
# 放在這裡，其他管理員才有辦法維護、重編，而不是鎖在某個人的家目錄裡。
SRCDIR      ?= $(PREFIX)/src/sysview
# 允許維護原始碼的群組（留空則只有 root 能改）
SRCGROUP    ?=
# 找 cargo：rustup 用 --no-modify-path 裝的話 ~/.cargo/bin 不會在 PATH 裡
CARGO       ?= $(shell command -v cargo 2>/dev/null || \
                       ls $(HOME)/.cargo/bin/cargo 2>/dev/null || \
                       echo cargo)
TARGETDIR   ?= target/release
# `make build` 完成時記下兩個執行檔的 sha256。install 用它確認要裝的東西
# 確實是「乾淨的 release build」，而不是被 cargo test --all-features 覆蓋過的版本。
#
# 記內容而不是記時間：cargo 是用 hardlink 把 deps/ 裡的產物接回 target/release/，
# 被換掉時 mtime 會跟著舊產物一起**倒退**，比時間根本抓不到。
STAMP        = $(TARGETDIR)/.plain-release.sha256

.PHONY: all build check-built test lint install install-user install-source uninstall clean verify verify-shared legacy help

all: build

help:
	@echo "make build      編譯 release 版"
	@echo "make test       跑全部測試"
	@echo "make lint       clippy + fmt 檢查"
	@echo "make install    安裝執行檔到 $(PREFIX)（需要 root）"
	@echo "make install-user    只裝給自己，裝到 ~/.local（不需要 root）"
	@echo "make install-source  另外把原始碼放到 $(SRCDIR)，讓其他管理員能維護"
	@echo "make verify     驗證安裝結果的權限是否安全"
	@echo "make uninstall  移除"
	@echo "make legacy     編譯 C++ v1（regression oracle）"

build:
	@command -v $(CARGO) >/dev/null 2>&1 || [ -x "$(CARGO)" ] || { \
	  echo "找不到 cargo。"; \
	  echo "  若你已用 rustup 安裝過，先執行：. \"$$HOME/.cargo/env\""; \
	  echo "  否則： curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"; \
	  exit 1; }
	$(CARGO) build --release --locked
	@sha256sum $(TARGETDIR)/sysview $(TARGETDIR)/sysview-priv > $(STAMP)

# 安裝前確認執行檔已經編好。刻意不自動編 —— 見 install 上方的說明。
check-built:
	@for b in sysview sysview-priv; do \
	  if [ ! -x "$(TARGETDIR)/$$b" ]; then \
	    echo "✗ 找不到 $(TARGETDIR)/$$b"; \
	    echo ""; \
	    echo "  install 不會自動編譯，因為 sudo 下的 root 找不到你的 cargo，"; \
	    echo "  而且以 root 編譯會在 target/ 留下 root 擁有的檔案。"; \
	    echo ""; \
	    echo "  請先以你自己的身分編譯，再安裝："; \
	    echo "      make            # 不要加 sudo"; \
	    echo "      sudo make install"; \
	    echo ""; \
	    echo "  或直接用 ./install.sh，它會照正確順序做完。"; \
	    exit 1; \
	  fi; \
	done
	@if [ ! -f "$(STAMP)" ]; then \
	    echo "✗ 找不到 $(STAMP)"; \
	    echo ""; \
	    echo "  這代表 target/ 裡的執行檔不是 make 建出來的。"; \
	    echo "  請先執行： make"; \
	    exit 1; \
	  fi
	@if ! sha256sum -c --status "$(STAMP)" 2>/dev/null; then \
	    echo "✗ $(TARGETDIR)/ 裡的執行檔跟 make 建出來的不一樣"; \
	    echo ""; \
	    echo "  有別的指令覆蓋過它了。最常見的是："; \
	    echo "      cargo test --all-features"; \
	    echo "  那會把啟用 test-support 的版本寫進 target/release/，"; \
	    echo "  裝上去的就不是正式版本了。"; \
	    echo ""; \
	    echo "  重新建一次再裝："; \
	    echo "      make            # 不要加 sudo"; \
	    echo "      sudo make install"; \
	    exit 1; \
	  fi
	@echo "==> 使用已編譯的執行檔（sha256 與 make 建出來的相符，不重新編譯）"

# 先把 v1 的 C++ oracle 備好，parity 的四項比對才真的會執行。
#
# 刻意是**非致命**的：oracle 的執行檔是建置產物、不進版控，而 sysview 本身
# 完全不需要 C++。這台機器沒有 g++ 就跳過比對（parity 測試自己會分辨
# 「編不出來」與「忘了編」），不要讓一個回歸比對基準擋住整個測試套件。
test:
	-@$(MAKE) --no-print-directory legacy 2>/dev/null || \
	  echo "  !  編不出 v1 oracle（需要 g++）；parity 比對會跳過"
	# --release 是必要的，不是為了快：tests/performance.rs 斷言的是
	# 「取樣 3 毫秒內、換頁 1 格內」這種**發行版的**時間預算。debug build
	# 慢一個數量級以上，那些測試必然失敗 —— 剛 clone 下來的人第一次跑
	# `make test` 就會看到一片紅，而程式其實好好的。
	$(CARGO) test --release --all-targets --all-features

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --all-features -- -D warnings

# 注意：install **不依賴** build。
#
# `sudo make install` 會以 root 執行，而 root 的 PATH 被 sudo 的 secure_path
# 清過，看不到使用者家目錄裡的 ~/.cargo/bin。就算看得到也不該用 root 編譯 ——
# 那會在 target/ 留下 root 擁有的檔案，之後使用者自己就編不動了。
#
# 正確順序是：先以自己的身分 `make`，再 `sudo make install`（只做複製）。
install: check-built
	install -d $(DESTDIR)$(BINDIR)
	install -d $(DESTDIR)$(LIBEXECDIR)
	install -d $(DESTDIR)$(DOCDIR)
	# 彩蛋排行榜的共用目錄：跟 /tmp 一樣的 1777（sticky）——
	# 每個人只能寫自己的 <uid>.json、讀得到別人的、動不了別人的。
	# sysview 自己不會建這個目錄（要 root）；沒有它就退回每人私人的榜。
	@if [ -n "$(SCOREDIR)" ]; then \
	   install -d -m 1777 "$(DESTDIR)$(SCOREDIR)" 2>/dev/null \
	     || echo "  !  建不出 $(SCOREDIR)（需要 root）；排行榜會退回每人私人的榜"; \
	 fi

	# 主程式：一般使用者執行
	install -m 0755 $(TARGETDIR)/sysview $(DESTDIR)$(BINDIR)/sysview
	# 特權 helper：0755 且**沒有 setuid**。
	# 提權的唯一途徑是 sudo，這樣授權才會經過 sudoers 而不是繞過它。
	install -m 0755 $(TARGETDIR)/sysview-priv $(DESTDIR)$(LIBEXECDIR)/sysview-priv
	install -m 0644 README.md $(DESTDIR)$(DOCDIR)/README.md
	install -m 0644 docs/sudo.md $(DESTDIR)$(DOCDIR)/sudo.md
	install -m 0644 docs/security.md $(DESTDIR)$(DOCDIR)/security.md
	install -m 0644 THIRD_PARTY_NOTICES.md $(DESTDIR)$(DOCDIR)/THIRD_PARTY_NOTICES.md
	@$(MAKE) --no-print-directory verify
	@$(MAKE) --no-print-directory verify-shared

# 把原始碼放到共用位置，讓其他管理員能維護與重編。
# 不含 target/ 與 .git/ —— 那些是建置產物與歷史，不需要佔空間。
# 使用者自己裝，**完全不需要 root**。
#
# 只裝主程式。helper 刻意不裝 —— `locate_helper()` 只認 root 擁有的系統路徑
# （見 src/privilege/client.rs 的 HELPER_CANDIDATES，且不接受環境變數覆寫），
# 裝在家目錄的 helper 永遠不會被採用。這是刻意的：sudo 去執行一個使用者
# 自己寫得動的檔案，等於把 root 送出去。所以家目錄安裝的結果是
# 「七個一般頁面全部可用，Admin 頁顯示 helper not installed」。
# 相依 build 而不是 check-built：`install` 不自動編是因為它跑在 sudo 底下
# （root 找不到你的 cargo，而且會在 target/ 留下 root 擁有的檔案）——
# install-user 完全沒有 root 參與，那個理由不成立。而且 `make test` 會把
# 帶 test-support 的版本寫進 target/release/，讓雜湊章對不上；相依 build
# 就會自動重建成乾淨的發行版，不用使用者自己想起來要再 make 一次。
install-user: build
	@install -d $(HOME)/.local/bin
	@install -d $(HOME)/.local/share/doc/sysview
	install -m 0755 $(TARGETDIR)/sysview $(HOME)/.local/bin/sysview
	@install -m 0644 README.md docs/sudo.md docs/security.md THIRD_PARTY_NOTICES.md $(HOME)/.local/share/doc/sysview/
	@echo "  ✓ $(HOME)/.local/bin/sysview"
	@case ":$$PATH:" in \
	   *":$(HOME)/.local/bin:"*) echo "  ✓ ~/.local/bin 已在 PATH 上";; \
	   *) echo "  !  ~/.local/bin 不在 PATH 上，加這行到 ~/.bashrc："; \
	      echo "       export PATH=\"$$HOME/.local/bin:$$PATH\"";; \
	 esac
	@echo "  !  Admin 頁不會啟用（helper 需要 root 安裝，見 make install）"

install-source:
	install -d $(DESTDIR)$(SRCDIR)
	@tar cf - --exclude=./target --exclude=./.git --exclude='*.tar.gz' . \
	  | tar xf - -C $(DESTDIR)$(SRCDIR)
	@if [ -n "$(SRCGROUP)" ]; then \
	    chgrp -R $(SRCGROUP) $(DESTDIR)$(SRCDIR); \
	    chmod -R g+w $(DESTDIR)$(SRCDIR); \
	    find $(DESTDIR)$(SRCDIR) -type d -exec chmod g+s {} + ; \
	    echo "  ✓ 原始碼 $(SRCDIR)（$(SRCGROUP) 群組可寫）"; \
	  else \
	    echo "  ✓ 原始碼 $(SRCDIR)（只有 root 可寫）"; \
	    echo "    要讓某個群組能維護：sudo make install-source SRCGROUP=<群組>"; \
	  fi

# 安裝後自我檢查：權限錯了要當場抓出來，而不是留一個提權漏洞在系統上
verify:
	@echo "==> 驗證安裝結果"
	@test -x $(DESTDIR)$(BINDIR)/sysview || { echo "✗ 找不到 sysview"; exit 1; }
	@test -x $(DESTDIR)$(LIBEXECDIR)/sysview-priv || { echo "✗ 找不到 sysview-priv"; exit 1; }
	@mode=$$(stat -c '%a' $(DESTDIR)$(LIBEXECDIR)/sysview-priv); \
	 case "$$mode" in \
	   4*|2*|6*) echo "✗ 嚴重：helper 帶有 setuid/setgid 位元 (mode $$mode)"; exit 1;; \
	 esac; \
	 case "$$mode" in \
	   *[2367]) echo "✗ 嚴重：helper 可被 group/other 寫入 (mode $$mode) — 等於任何人都能拿 root"; exit 1;; \
	 esac; \
	 echo "  ✓ helper mode $$mode（無 setuid、不可被他人寫入）"
	@owner=$$(stat -c '%U:%G' $(DESTDIR)$(LIBEXECDIR)/sysview-priv); \
	 echo "  ✓ helper 擁有者 $$owner"
	@echo "  ✓ 安裝檢查通過"

# 檢查「機器上每個使用者」是不是真的都能執行。
# 這是這個工具的核心需求，不能只靠假設。
verify-shared:
	@echo "==> 驗證共用可用性"
	@bin="$(BINDIR)/sysview"; \
	 if [ -n "$(DESTDIR)" ] && [ ! -e "$$bin" ]; then \
	   echo "  (DESTDIR 暫存安裝，改為檢查最終路徑 $$bin 的祖先目錄)"; \
	 fi; \
	 p="$$bin"; bad=""; \
	 while [ "$$p" != "/" ] && [ -n "$$p" ]; do \
	   if [ -e "$$p" ]; then \
	     perm=$$(stat -c '%a' "$$p"); \
	     case "$$perm" in \
	       *[1357]) ;; \
	       *) bad="$$p (mode $$perm)"; break;; \
	     esac; \
	   fi; \
	   p=$$(dirname "$$p"); \
	 done; \
	 if [ -n "$$bad" ]; then \
	   echo "  ✗ 路徑上有目錄不開放給其他使用者：$$bad"; \
	   echo "    其他帳號將無法執行 sysview"; exit 1; \
	 fi; \
	 echo "  ✓ $$bin 到根目錄的整條路徑對所有使用者開放"
	@echo "  ✓ 一般使用者：全部監控功能可用，完全不需要 sudo"
	@echo "  ✓ 沒有 sudo 權限者：一樣能用，只是 Admin 頁會顯示 not authorized"
	@case "$(BINDIR)" in \
	   /usr/local/bin|/usr/bin|/bin) \
	     echo "  ✓ $(BINDIR) 在預設 PATH 上，所有人直接打 sysview 就能用";; \
	   *) echo "  !  $(BINDIR) 不在預設 PATH 上，使用者需要自己加";; \
	 esac
	@echo "  ✓ 執行期只需要 libc/libgcc，不需要 Rust 工具鏈"

uninstall:
	rm -f $(HOME)/.local/bin/sysview
	rm -rf $(HOME)/.local/share/doc/sysview
	rm -f $(DESTDIR)$(BINDIR)/sysview
	rm -f $(DESTDIR)$(LIBEXECDIR)/sysview-priv
	rmdir $(DESTDIR)$(LIBEXECDIR) 2>/dev/null || true
	rm -rf $(DESTDIR)$(DOCDIR)
	rm -rf $(DESTDIR)$(SRCDIR)

clean:
	$(CARGO) clean
	$(MAKE) -C legacy/cpp clean 2>/dev/null || true

legacy:
	$(MAKE) -C legacy/cpp
