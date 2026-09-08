import sys, unicodedata
def cw(ch):
    if unicodedata.combining(ch): return 0
    return 2 if unicodedata.east_asian_width(ch) in ('W','F') else 1
def cells(line):
    """把 pyte 的輸出還原成 (字元, 佔幾格) 序列：全形字後面那個填充空白要吃掉。"""
    out=[]; i=0
    while i < len(line):
        ch=line[i]; w=cw(ch)
        out.append(ch)
        i+=1
        if w==2 and i<len(line) and line[i]==' ':
            i+=1   # 跳過 pyte 的填充格
    return out
def slice_cols(line,a,b):
    out=[]; col=0
    for ch in cells(line):
        w=cw(ch)
        if col>=a and col+w<=b: out.append(ch)
        col+=w
        if col>=b: break
    return "".join(out)
name=sys.argv[1]; a=int(sys.argv[2]); b=int(sys.argv[3])
lines=open("v2_shots.txt").read().split("\n")
start=next((i for i,l in enumerate(lines) if l.startswith("█") and name in l), None)
if start is None: print("找不到",name); sys.exit(1)
end=next((i for i in range(start+1,len(lines)) if lines[i].startswith("█")), len(lines))
for l in lines[start:end]: print(slice_cols(l,a,b))
