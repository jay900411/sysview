import os,pty,sys,time,fcntl,termios,struct,select,signal,pyte

# 從這個腳本的位置往上兩層找 repo 根，而不是寫死某個人的家目錄 ——
# 寫死的話別人 clone 下來一跑就是 FileNotFoundError。
# 也可以用 SYSVIEW_BIN 指定別的執行檔。
_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.environ.get("SYSVIEW_BIN", os.path.join(_ROOT, "target", "release", "sysview"))
if not os.path.isfile(BIN):
    sys.exit(f"找不到 {BIN}\n請先執行 `make build`，或用 SYSVIEW_BIN 指定執行檔。")

def disp(sc):
    out=[]
    for y in range(sc.lines):
        row=sc.buffer[y]
        out.append("".join((row[x].data or " ") for x in range(sc.columns)).rstrip())
    return "\n".join(out)

def run(keys, cols=190, rows=48, warm=14.0, extra_env=None):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"]="xterm-256color"; os.environ["LANG"]="en_US.UTF-8"
        os.environ["COLORTERM"]="truecolor"
        os.environ["COLUMNS"]=str(cols); os.environ["LINES"]=str(rows)
        if extra_env: os.environ.update(extra_env)
        os.execv(BIN,[BIN,"-i","0.4"])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    screen=pyte.Screen(cols,rows); stream=pyte.ByteStream(screen)
    nbytes=0
    def pump(t):
        nonlocal nbytes
        end=time.time()+t
        while time.time()<end:
            r,_,_=select.select([fd],[],[],0.05)
            if r:
                try: d=os.read(fd,1<<20)
                except OSError: return False
                if not d: return False
                nbytes+=len(d); stream.feed(d)
        return True
    pump(warm)
    shots=[]
    for label,k in keys:
        if k: os.write(fd,k)
        pump(1.2)
        shots.append((label,disp(screen)))
    # 統計資源
    try:
        st=open("/proc/%d/stat"%pid).read(); f=st[st.rindex(")")+2:].split()
        HZ=os.sysconf("SC_CLK_TCK")
        cpu=(int(f[11])+int(f[12]))/HZ; kids=(int(f[13])+int(f[14]))/HZ
        rss=int(f[21])*os.sysconf("SC_PAGE_SIZE")
    except Exception: cpu=kids=rss=0
    os.write(fd,b"q"); pump(0.3)
    try: os.kill(pid,signal.SIGKILL)
    except OSError: pass
    os.close(fd); os.waitpid(pid,0)
    return shots, dict(cpu=cpu,kids=kids,rss=rss,bytes=nbytes)

views=[("總覽 overview",None),("1 CPU",b"1"),("2 記憶體",b"2"),("3 GPU",b"3"),
       ("4 儲存",b"4"),("5 網路",b"5"),("6 行程",b"6"),("A 管理員",b"A"),
       ("e Explain",b"1e"),("? 說明",b"?"),("t 換主題",b"qt")]
shots,stats=run(views)
buf=[]
for label,s in shots:
    buf.append("\n"+"█"*28+"  "+label+"  "+"█"*28)
    buf.append(s.rstrip())
txt="\n".join(buf)
open("v2_shots.txt","w").write(txt)
bad=[l for l in txt.splitlines() if "panic" in l.lower() or "RUST_BACKTRACE" in l or "thread '" in l]
print("擷取 %d 張畫面，panic/錯誤行: %d" % (len(shots),len(bad)))
for b in bad[:5]: print("  !!",b)
print("自身 CPU %.3fs  子行程 %.3fs  RSS %.1f MB  終端輸出 %.1f KB" %
      (stats["cpu"],stats["kids"],stats["rss"]/1048576,stats["bytes"]/1024))
