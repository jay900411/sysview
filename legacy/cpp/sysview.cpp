// sysview — 一體式系統儀表板 (CPU / 記憶體 / GPU / 儲存 / 網路 / 行程)
//
// 純 C++17，零外部相依（不用 ncurses，自己做 termios + ANSI 雙緩衝差分渲染）。
// 只讀 /proc 與 /sys，不需要 root，所有使用者皆可執行。
//
//   g++ -O2 -std=c++17 -o sysview sysview.cpp -lpthread

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <unordered_map>
#include <vector>

#include <dirent.h>
#include <fcntl.h>
#include <poll.h>
#include <pwd.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <ifaddrs.h>
#include <netdb.h>
#include <netinet/in.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

static const char* VERSION = "1.0";
static const size_t HIST = 240;

// ─────────────────────────────────────────────────────────────────────────────
// 基本工具
// ─────────────────────────────────────────────────────────────────────────────

static long CLK_TCK_ = sysconf(_SC_CLK_TCK);
static long PAGESZ_  = sysconf(_SC_PAGE_SIZE);

static std::string fmt(const char* f, ...) {
    char buf[1024];
    va_list ap; va_start(ap, f);
    int n = vsnprintf(buf, sizeof buf, f, ap);
    va_end(ap);
    if (n < 0) return "";
    if ((size_t)n < sizeof buf) return std::string(buf, n);
    std::string s((size_t)n, '\0');
    va_start(ap, f); vsnprintf(&s[0], (size_t)n + 1, f, ap); va_end(ap);
    return s;
}

// 讀整個小檔案。/proc 的檔案 stat 出來是 0 bytes，所以只能一直 read 到 EOF。
static bool readFile(const char* path, std::string& out) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return false;
    out.clear();
    char buf[8192];
    ssize_t n;
    while ((n = read(fd, buf, sizeof buf)) > 0) out.append(buf, (size_t)n);
    close(fd);
    return true;
}
static std::string rd(const std::string& path) {
    std::string s;
    readFile(path.c_str(), s);
    return s;
}
static std::string rdTrim(const std::string& path) {
    std::string s = rd(path);
    size_t a = s.find_first_not_of(" \t\r\n");
    if (a == std::string::npos) return "";
    size_t b = s.find_last_not_of(" \t\r\n");
    return s.substr(a, b - a + 1);
}
static bool rdLong(const std::string& path, long long& out) {
    std::string s = rdTrim(path);
    if (s.empty()) return false;
    char* end = nullptr;
    long long v = strtoll(s.c_str(), &end, 10);
    if (end == s.c_str()) return false;
    out = v;
    return true;
}
static long long rdLongOr(const std::string& path, long long dflt) {
    long long v;
    return rdLong(path, v) ? v : dflt;
}

static double nowMono() {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + ts.tv_nsec / 1e9;
}

template <class T> static T clampv(T v, T lo, T hi) { return v < lo ? lo : (v > hi ? hi : v); }

// 位元組 -> 人類可讀
static std::string human(double n, bool valid = true) {
    if (!valid) return "  --  ";
    const char* suf[] = {"", "K", "M", "G", "T", "P"};
    bool neg = n < 0;
    n = std::fabs(n);
    int i = 0;
    while (n >= 1024.0 && i < 5) { n /= 1024.0; i++; }
    std::string s;
    if (i == 0) s = fmt("%.0f B", n);
    else        s = fmt("%.*f %sB", n < 100 ? 1 : 0, n, suf[i]);
    return neg ? "-" + s : s;
}
static std::string hzs(double mhz, bool valid = true) {
    if (!valid) return " -- ";
    if (mhz >= 1000.0) return fmt("%.2f GHz", mhz / 1000.0);
    return fmt("%.0f MHz", mhz);
}
static std::string durs(double sec) {
    long long t = (long long)sec;
    long long d = t / 86400; t %= 86400;
    long long h = t / 3600;  t %= 3600;
    long long m = t / 60,  s = t % 60;
    if (d) return fmt("%lldd %02lld:%02lld", d, h, m);
    return fmt("%02lld:%02lld:%02lld", h, m, s);
}

static std::vector<std::string> splitWS(const std::string& s) {
    std::vector<std::string> v;
    size_t i = 0, n = s.size();
    while (i < n) {
        while (i < n && (s[i] == ' ' || s[i] == '\t')) i++;
        size_t a = i;
        while (i < n && s[i] != ' ' && s[i] != '\t') i++;
        if (i > a) v.emplace_back(s, a, i - a);
    }
    return v;
}
// 逐行走訪，避免產生一大堆暫時字串
template <class F> static void eachLine(const std::string& s, F f) {
    size_t a = 0;
    while (a < s.size()) {
        size_t b = s.find('\n', a);
        if (b == std::string::npos) b = s.size();
        f(std::string_view(s.data() + a, b - a));
        a = b + 1;
    }
}
static bool startsWith(std::string_view s, std::string_view p) {
    return s.size() >= p.size() && memcmp(s.data(), p.data(), p.size()) == 0;
}

// ─────────────────────────────────────────────────────────────────────────────
// UTF-8 與終端機顯示寬度（中日韓全形字佔 2 格）
// ─────────────────────────────────────────────────────────────────────────────

// 解出一個碼位，回傳它佔的位元組數
static int utf8Decode(const char* s, size_t len, uint32_t& cp) {
    unsigned char c = (unsigned char)s[0];
    if (c < 0x80) { cp = c; return 1; }
    int n; uint32_t v;
    if      ((c & 0xE0) == 0xC0) { n = 2; v = c & 0x1F; }
    else if ((c & 0xF0) == 0xE0) { n = 3; v = c & 0x0F; }
    else if ((c & 0xF8) == 0xF0) { n = 4; v = c & 0x07; }
    else { cp = 0xFFFD; return 1; }
    if (len < (size_t)n) { cp = 0xFFFD; return 1; }
    for (int i = 1; i < n; i++) {
        unsigned char b = (unsigned char)s[i];
        if ((b & 0xC0) != 0x80) { cp = 0xFFFD; return 1; }
        v = (v << 6) | (b & 0x3F);
    }
    cp = v;
    return n;
}

struct Range { uint32_t lo, hi; };
// East Asian Wide / Fullwidth。刻意不含 Ambiguous（↑↓▶█▏· 之類），
// 因為現代終端機一律把 Ambiguous 畫成半形；這跟 Python 版驗證過的行為一致。
static const Range WIDE[] = {
    {0x1100,0x115F},{0x231A,0x231B},{0x2329,0x232A},{0x23E9,0x23EC},{0x23F0,0x23F0},
    {0x23F3,0x23F3},{0x25FD,0x25FE},{0x2614,0x2615},{0x2648,0x2653},{0x267F,0x267F},
    {0x2693,0x2693},{0x26A1,0x26A1},{0x26AA,0x26AB},{0x26BD,0x26BE},{0x26C4,0x26C5},
    {0x26CE,0x26CE},{0x26D4,0x26D4},{0x26EA,0x26EA},{0x26F2,0x26F3},{0x26F5,0x26F5},
    {0x26FA,0x26FA},{0x26FD,0x26FD},{0x2705,0x2705},{0x270A,0x270B},{0x2728,0x2728},
    {0x274C,0x274C},{0x274E,0x274E},{0x2753,0x2755},{0x2757,0x2757},{0x2795,0x2797},
    {0x27B0,0x27B0},{0x27BF,0x27BF},{0x2B1B,0x2B1C},{0x2B50,0x2B50},{0x2B55,0x2B55},
    {0x2E80,0x303E},{0x3041,0x33FF},{0x3400,0x4DBF},{0x4E00,0x9FFF},{0xA000,0xA4CF},
    {0xA960,0xA97F},{0xAC00,0xD7A3},{0xF900,0xFAFF},{0xFE10,0xFE19},{0xFE30,0xFE52},
    {0xFE54,0xFE66},{0xFE68,0xFE6B},{0xFF01,0xFF60},{0xFFE0,0xFFE6},
    {0x16FE0,0x16FE4},{0x17000,0x187F7},{0x18800,0x18CD5},{0x1B000,0x1B152},
    {0x1F004,0x1F004},{0x1F0CF,0x1F0CF},{0x1F18E,0x1F18E},{0x1F191,0x1F19A},
    {0x1F200,0x1F320},{0x1F32D,0x1F335},{0x1F337,0x1F37C},{0x1F37E,0x1F393},
    {0x1F3A0,0x1F3CA},{0x1F3CF,0x1F3D3},{0x1F3E0,0x1F3F0},{0x1F3F4,0x1F3F4},
    {0x1F3F8,0x1F43E},{0x1F440,0x1F440},{0x1F442,0x1F4FC},{0x1F4FF,0x1F53D},
    {0x1F54B,0x1F54E},{0x1F550,0x1F567},{0x1F57A,0x1F57A},{0x1F595,0x1F596},
    {0x1F5A4,0x1F5A4},{0x1F5FB,0x1F64F},{0x1F680,0x1F6C5},{0x1F6CC,0x1F6CC},
    {0x1F6D0,0x1F6D2},{0x1F6EB,0x1F6EC},{0x1F6F4,0x1F6FC},{0x1F7E0,0x1F7EB},
    {0x1F90C,0x1F9FF},{0x1FA70,0x1FAF6},
    {0x20000,0x2FFFD},{0x30000,0x3FFFD},
};

static int cpWidth(uint32_t cp) {
    if (cp < 0x1100) return 1;
    // 組合字元不佔格
    if ((cp >= 0x0300 && cp <= 0x036F) || (cp >= 0x200B && cp <= 0x200F) ||
        cp == 0xFEFF || (cp >= 0xFE00 && cp <= 0xFE0F)) return 0;
    int lo = 0, hi = (int)(sizeof(WIDE) / sizeof(WIDE[0])) - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (cp < WIDE[mid].lo) hi = mid - 1;
        else if (cp > WIDE[mid].hi) lo = mid + 1;
        else return 2;
    }
    return 1;
}

// 字串在終端機上佔幾格
static int dw(std::string_view s) {
    int w = 0;
    size_t i = 0;
    while (i < s.size()) {
        uint32_t cp;
        int n = utf8Decode(s.data() + i, s.size() - i, cp);
        w += cpWidth(cp);
        i += (size_t)n;
    }
    return w;
}

// 裁到最多 w 格（不加省略號）
static std::string dcut(std::string_view s, int w) {
    if (w <= 0) return "";
    int acc = 0;
    size_t i = 0;
    while (i < s.size()) {
        uint32_t cp;
        int n = utf8Decode(s.data() + i, s.size() - i, cp);
        int cw = cpWidth(cp);
        if (acc + cw > w) break;
        acc += cw;
        i += (size_t)n;
    }
    return std::string(s.substr(0, i));
}
// 裁到 w 格，被裁到就補省略號
static std::string trunc(std::string_view s, int w) {
    if (w <= 0) return "";
    if (dw(s) <= w) return std::string(s);
    if (w == 1) return "…";
    return dcut(s, w - 1) + "…";
}
// 補空白到剛好 w 格
static std::string dpad(std::string_view s, int w, bool right = false) {
    std::string t = dcut(s, w);
    int pad = w - dw(t);
    if (pad <= 0) return t;
    return right ? std::string((size_t)pad, ' ') + t : t + std::string((size_t)pad, ' ');
}

// ─────────────────────────────────────────────────────────────────────────────
// 調色盤
// ─────────────────────────────────────────────────────────────────────────────

enum Col {
    C_FG, C_DIM, C_FAINT, C_FRAME, C_TITLE, C_ACCENT, C_WHITE,
    C_CPU, C_MEM, C_GPU, C_DISK, C_NET, C_PROC,
    C_OK, C_WARN, C_CRIT, C_COOL, C_NCOL
};
// 256 色索引
static const int PAL256[C_NCOL] = {
    252, 245, 240, 238, 111, 111, 255,
    39, 141, 84, 215, 44, 180,
    76, 214, 203, 38
};
// 退化到 8 色時的對應（30-37）
static const int PAL8[C_NCOL] = {
    37, 37, 34, 34, 36, 36, 37,
    36, 35, 32, 33, 36, 33,
    32, 33, 31, 36
};
static bool g_has256 = true;

static const int A_BOLD = 1, A_REV = 2, A_UL = 4;

// 依百分比選冷/暖/熱
static Col heat(double pct) {
    if (pct >= 90) return C_CRIT;
    if (pct >= 70) return C_WARN;
    if (pct >= 40) return C_OK;
    return C_COOL;
}

// ─────────────────────────────────────────────────────────────────────────────
// 螢幕：雙緩衝 + 差分輸出。只把真的變了的格子寫到終端機。
// ─────────────────────────────────────────────────────────────────────────────

struct Cell {
    // 大部分格子是單一 ASCII 字元，直接內嵌避免配置記憶體
    char  b[5] = {' ', 0, 0, 0, 0};
    uint8_t len = 1;
    uint8_t col = C_FG;
    uint8_t at  = 0;
    bool   cont = false;   // 全形字的右半格

    bool same(const Cell& o) const {
        return cont == o.cont && col == o.col && at == o.at &&
               len == o.len && memcmp(b, o.b, len) == 0;
    }
    void set(const char* s, int n, uint8_t c, uint8_t a) {
        if (n > 4) n = 4;
        memcpy(b, s, (size_t)n); len = (uint8_t)n; col = c; at = a; cont = false;
    }
    void blank() { b[0] = ' '; len = 1; col = C_FG; at = 0; cont = false; }
};

class Screen {
public:
    int H = 0, W = 0;

    void resize(int h, int w) {
        if (h == H && w == W) return;
        H = h; W = w;
        cur.assign((size_t)H * W, Cell());
        prev.assign((size_t)H * W, Cell());
        for (auto& c : prev) c.len = 0;      // 強制第一次全畫
        out.reserve(1 << 16);
    }
    void clear() { for (auto& c : cur) c.blank(); }

    inline Cell& at(int y, int x) { return cur[(size_t)y * W + x]; }

    // 在 (y,x) 寫入字串，超出邊界自動裁掉
    void put(int y, int x, std::string_view s, Col color = C_FG, int attr = 0) {
        if (y < 0 || y >= H || s.empty()) return;
        size_t i = 0;
        while (i < s.size()) {
            uint32_t cp;
            int n = utf8Decode(s.data() + i, s.size() - i, cp);
            int cw = cpWidth(cp);
            if (cw == 0) { i += (size_t)n; continue; }
            if (x >= W) break;
            if (x + cw > W) break;              // 全形字塞不下就不畫，免得跨行
            if (x >= 0) {
                clearOverlap(y, x, cw);
                Cell& c = at(y, x);
                c.set(s.data() + i, n, (uint8_t)color, (uint8_t)attr);
                if (cw == 2) {
                    Cell& c2 = at(y, x + 1);
                    c2.blank(); c2.cont = true; c2.col = (uint8_t)color; c2.at = (uint8_t)attr;
                }
            }
            x += cw;
            i += (size_t)n;
        }
    }

    // 把差分結果吐到終端機
    void flush(int fd) {
        out.clear();
        int lastY = -1, lastX = -1, lastCol = -1, lastAt = -1;
        for (int y = 0; y < H; y++) {
            for (int x = 0; x < W; x++) {
                size_t i = (size_t)y * W + x;
                Cell& c = cur[i];
                if (c.same(prev[i])) continue;
                prev[i] = c;
                if (c.cont) continue;          // 右半格由左半格一起畫掉了
                if (y != lastY || x != lastX) {
                    out += fmt("\x1b[%d;%dH", y + 1, x + 1);
                    lastY = y; lastX = x;
                }
                if (c.col != lastCol || c.at != lastAt) {
                    sgr(c.col, c.at);
                    lastCol = c.col; lastAt = c.at;
                }
                out.append(c.b, c.len);
                lastX += (c.len > 1 && cellWide(c)) ? 2 : 1;
            }
        }
        if (!out.empty()) {
            out += "\x1b[0m";
            ssize_t unused = write(fd, out.data(), out.size());
            (void)unused;
        }
    }
    void forceRedraw() { for (auto& c : prev) c.len = 0; }

private:
    std::vector<Cell> cur, prev;
    std::string out;

    static bool cellWide(const Cell& c) {
        uint32_t cp;
        utf8Decode(c.b, c.len, cp);
        return cpWidth(cp) == 2;
    }
    // 蓋掉別人半個全形字時，把對方殘留的另一半清成空白
    void clearOverlap(int y, int x, int cw) {
        Cell& c = at(y, x);
        if (c.cont && x > 0) at(y, x - 1).blank();
        if (!c.cont && c.len && cellWide(c) && x + 1 < W) at(y, x + 1).blank();
        if (cw == 2 && x + 1 < W) {
            Cell& n = at(y, x + 1);
            if (!n.cont && n.len && cellWide(n) && x + 2 < W) at(y, x + 2).blank();
        }
    }
    void sgr(int col, int at_) {
        out += "\x1b[0";
        if (at_ & A_BOLD) out += ";1";
        if (at_ & A_UL)   out += ";4";
        if (at_ & A_REV)  out += ";7";
        if (g_has256) out += fmt(";38;5;%d", PAL256[col]);
        else          out += fmt(";%d", PAL8[col]);
        out += "m";
    }
};

// ─────────────────────────────────────────────────────────────────────────────
// 繪圖原語
// ─────────────────────────────────────────────────────────────────────────────

static const char* BLOCKS[9] = {" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"};
static const char* SPARK[8]  = {"▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"};

// 圓角外框，標題嵌在上緣，副標題靠右
static void frameBox(Screen& S, int y, int x, int h, int w,
                     const std::string& title = "", Col color = C_FRAME,
                     Col tcolor = C_TITLE, const std::string& sub = "") {
    if (h < 2 || w < 2) return;
    std::string top = "╭", bot = "╰";
    for (int i = 0; i < w - 2; i++) { top += "─"; bot += "─"; }
    top += "╮"; bot += "╯";
    S.put(y, x, top, color);
    S.put(y + h - 1, x, bot, color);
    for (int i = 1; i < h - 1; i++) {
        S.put(y + i, x, "│", color);
        S.put(y + i, x + w - 1, "│", color);
    }
    if (!title.empty()) {
        std::string t = trunc(title, std::max(0, w - 6));
        S.put(y, x + 2, "┤ ", color);
        S.put(y, x + 4, t, tcolor, A_BOLD);
        S.put(y, x + 4 + dw(t), " ├", color);
    }
    if (!sub.empty()) {
        std::string s2 = trunc(sub, std::max(0, w - 8 - dw(title) - 4));
        if (!s2.empty()) S.put(y, x + w - 5 - dw(s2), "┤ " + s2 + " ├", C_FAINT);
    }
}

// 漸層填充條，支援 1/8 格細分
static void bar(Screen& S, int y, int x, int w, double pct, int color = -1) {
    if (w <= 0) return;
    pct = clampv(std::isnan(pct) ? 0.0 : pct, 0.0, 100.0);
    Col col = (color < 0) ? heat(pct) : (Col)color;
    double filled = pct / 100.0 * w;
    int full = (int)filled;
    double frac = filled - full;
    std::string s;
    for (int i = 0; i < full && i < w; i++) s += "█";
    int used = std::min(full, w);
    if (used < w && frac > 0.06) {
        s += BLOCKS[clampv((int)(frac * 8), 1, 8)];
        used++;
    }
    if (!s.empty()) S.put(y, x, s, col, A_BOLD);
    for (int i = used; i < w; i++) S.put(y, x + i, "·", C_FRAME);
}

using Ring = std::deque<double>;
static void push(Ring& r, double v) { r.push_back(v); if (r.size() > HIST) r.pop_front(); }

// 單行區塊圖
static std::string sparkline(const Ring& r, int w) {
    if (w <= 0) return "";
    int n = (int)r.size();
    int start = std::max(0, n - w);
    double top = 0;
    for (int i = start; i < n; i++) top = std::max(top, r[(size_t)i]);
    if (top <= 0) top = 1e-9;
    std::string s;
    for (int i = 0; i < w - (n - start); i++) s += " ";
    for (int i = start; i < n; i++) {
        int k = (int)(clampv(r[(size_t)i] / top, 0.0, 1.0) * 7);
        s += SPARK[k];
    }
    return s;
}

// 點陣面積圖：每格 2 欄 x 4 列
static const uint8_t BR[2][4] = {{0x01, 0x02, 0x04, 0x40}, {0x08, 0x10, 0x20, 0x80}};

static void drawGraph(Screen& S, int y, int x, int w, int h, const Ring& r,
                      Col color, double vmax = 0, bool axis = true) {
    if (w <= 0 || h <= 0) return;
    int gw = (axis && w > 18) ? w - 6 : w;
    if (gw <= 0) return;
    int dwid = gw * 2, dhgt = h * 4;
    int n = (int)r.size();
    int start = std::max(0, n - dwid);
    double top = vmax;
    if (top <= 0) { for (int i = start; i < n; i++) top = std::max(top, r[(size_t)i]); }
    if (top <= 0) top = 1e-9;

    std::vector<uint8_t> grid((size_t)h * gw, 0);
    int off = dwid - (n - start);
    for (int i = start; i < n; i++) {
        int col = off + (i - start);
        if (col < 0 || col >= dwid) continue;
        double v = r[(size_t)i];
        int lvl = (int)std::lround(clampv(v / top, 0.0, 1.0) * dhgt);
        if (lvl <= 0 && v > 0) lvl = 1;
        for (int drow = dhgt - lvl; drow < dhgt; drow++) {
            if (drow < 0) continue;
            int cy = drow / 4, ry = drow % 4;
            int cx = col / 2, rx = col % 2;
            grid[(size_t)cy * gw + cx] |= BR[rx][ry];
        }
    }
    std::string line;
    for (int i = 0; i < h; i++) {
        line.clear();
        for (int j = 0; j < gw; j++) {
            uint32_t cp = 0x2800u + grid[(size_t)i * gw + j];
            char b[4];
            b[0] = (char)(0xE0 | (cp >> 12));
            b[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
            b[2] = (char)(0x80 | (cp & 0x3F));
            line.append(b, 3);
        }
        S.put(y + i, x, line, color);
    }
    if (axis && w > 18) {
        auto axfmt = [](double v) {
            if (v >= 1000) return fmt("%.0fk", v / 1000.0);
            if (v >= 10)   return fmt("%.0f", v);
            return fmt("%.1f", v);
        };
        S.put(y, x + gw + 1, dpad(axfmt(top), 5, true), C_FAINT);
        if (h >= 3) S.put(y + h / 2, x + gw + 1, dpad(axfmt(top / 2), 5, true), C_FAINT);
        S.put(y + h - 1, x + gw + 1, dpad(axfmt(0), 5, true), C_FAINT);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 取樣器：只讀 /proc 與 /sys，不需要 root
// ─────────────────────────────────────────────────────────────────────────────

struct Cpu {
    int n = 0;
    std::vector<long long> prevTot, prevIdle;
    long long pTot = 0, pIdle = 0;
    long long pIow = 0, pSteal = 0;
    Ring total;
    std::vector<Ring> cores;
    std::vector<double> corePct, freqs;
    double pct = 0, iowait = 0, steal = 0;
    std::string model = "unknown";
    double load[3] = {0, 0, 0};
    double temp = -1;
    std::map<int, double> coreTemps;
    double ctxtRate = 0, intrRate = 0, forkRate = 0;
    long long pCtxt = -1, pIntr = -1, pFork = -1;
    int running = 0, blocked = 0;
    double uptime = 0;
    std::string hwmon;

    Cpu() {
        std::string ci = rd("/proc/cpuinfo");
        eachLine(ci, [&](std::string_view l) {
            if (model == "unknown" && startsWith(l, "model name")) {
                size_t c = l.find(':');
                if (c != std::string_view::npos) {
                    std::string_view v = l.substr(c + 1);
                    while (!v.empty() && v.front() == ' ') v.remove_prefix(1);
                    model = std::string(v);
                }
            }
        });
        findHwmon();
    }

    void findHwmon() {
        DIR* d = opendir("/sys/class/hwmon");
        if (!d) return;
        std::vector<std::string> names;
        while (dirent* e = readdir(d)) {
            if (e->d_name[0] == '.') continue;
            names.emplace_back(e->d_name);
        }
        closedir(d);
        std::sort(names.begin(), names.end());
        for (auto& nm : names) {
            std::string p = "/sys/class/hwmon/" + nm;
            std::string t = rdTrim(p + "/name");
            if (t == "coretemp" || t == "k10temp" || t == "zenpower") { hwmon = p; return; }
        }
    }

    void readTemps() {
        coreTemps.clear();
        temp = -1;
        if (hwmon.empty()) {
            long long v = rdLongOr("/sys/class/thermal/thermal_zone0/temp", -1);
            if (v > 0) temp = v / 1000.0;
            return;
        }
        DIR* d = opendir(hwmon.c_str());
        if (!d) return;
        double pkg = -1, mx = -1;
        while (dirent* e = readdir(d)) {
            const char* nm = e->d_name;
            size_t L = strlen(nm);
            if (L < 11 || strncmp(nm, "temp", 4) != 0) continue;
            if (strcmp(nm + L - 6, "_label") != 0) continue;
            std::string idx(nm, L - 6);
            std::string label = rdTrim(hwmon + "/" + nm);
            long long v = rdLongOr(hwmon + "/" + idx + "_input", -1);
            if (v < 0) continue;
            double c = v / 1000.0;
            if (label.find("Package") != std::string::npos ||
                label.find("Tdie") != std::string::npos ||
                label.find("Tctl") != std::string::npos) {
                pkg = c;
            } else {
                size_t dpos = label.find_first_of("0123456789");
                int ci2 = (dpos == std::string::npos) ? (int)coreTemps.size()
                                                      : atoi(label.c_str() + dpos);
                coreTemps[ci2] = c;
                mx = std::max(mx, c);
            }
        }
        closedir(d);
        temp = (pkg >= 0) ? pkg : mx;
    }

    void update(double dt) {
        std::string st = rd("/proc/stat");
        std::vector<long long> tots, idles;
        long long ctxt = -1, intr = -1, fork = -1;
        eachLine(st, [&](std::string_view l) {
            if (startsWith(l, "cpu")) {
                auto f = splitWS(std::string(l));
                if (f.size() < 5) return;
                long long tot = 0;
                for (size_t i = 1; i < f.size(); i++) tot += atoll(f[i].c_str());
                long long idle = atoll(f[4].c_str());
                if (f.size() > 5) idle += atoll(f[5].c_str());   // + iowait
                if (f[0] == "cpu") {
                    long long iow = (f.size() > 5) ? atoll(f[5].c_str()) : 0;
                    long long stl = (f.size() > 8) ? atoll(f[8].c_str()) : 0;
                    long long dT = tot - pTot;
                    if (pTot && dT > 0) {
                        pct = clampv((dT - (idle - pIdle)) * 100.0 / dT, 0.0, 100.0);
                        push(total, pct);
                        iowait = (iow - pIow) * 100.0 / dT;
                        steal  = (stl - pSteal) * 100.0 / dT;
                    }
                    pTot = tot; pIdle = idle; pIow = iow; pSteal = stl;
                } else {
                    tots.push_back(tot); idles.push_back(idle);
                }
            } else if (startsWith(l, "ctxt "))          ctxt = atoll(l.data() + 5);
            else if (startsWith(l, "intr "))            intr = atoll(l.data() + 5);
            else if (startsWith(l, "processes "))       fork = atoll(l.data() + 10);
            else if (startsWith(l, "procs_running "))   running = atoi(l.data() + 14);
            else if (startsWith(l, "procs_blocked "))   blocked = atoi(l.data() + 14);
        });
        if ((int)tots.size() != n) {
            n = (int)tots.size();
            cores.assign((size_t)n, Ring());
            corePct.assign((size_t)n, 0.0);
            prevTot.assign((size_t)n, 0);
            prevIdle.assign((size_t)n, 0);
        }
        for (int i = 0; i < n; i++) {
            long long dT = tots[(size_t)i] - prevTot[(size_t)i];
            if (prevTot[(size_t)i] && dT > 0) {
                double p = clampv((dT - (idles[(size_t)i] - prevIdle[(size_t)i])) * 100.0 / dT,
                                  0.0, 100.0);
                corePct[(size_t)i] = p;
                push(cores[(size_t)i], p);
            }
            prevTot[(size_t)i] = tots[(size_t)i];
            prevIdle[(size_t)i] = idles[(size_t)i];
        }
        if (dt > 0) {
            if (pCtxt >= 0 && ctxt >= 0) ctxtRate = (ctxt - pCtxt) / dt;
            if (pIntr >= 0 && intr >= 0) intrRate = (intr - pIntr) / dt;
            if (pFork >= 0 && fork >= 0) forkRate = (fork - pFork) / dt;
        }
        pCtxt = ctxt; pIntr = intr; pFork = fork;

        freqs.assign((size_t)n, -1.0);
        bool any = false;
        for (int i = 0; i < n; i++) {
            long long v = rdLongOr(fmt("/sys/devices/system/cpu/cpu%d/cpufreq/scaling_cur_freq", i), -1);
            if (v > 0) { freqs[(size_t)i] = v / 1000.0; any = true; }
        }
        if (!any) {
            int i = 0;
            std::string ci = rd("/proc/cpuinfo");
            eachLine(ci, [&](std::string_view l) {
                if (startsWith(l, "cpu MHz") && i < n) {
                    size_t c = l.find(':');
                    if (c != std::string_view::npos) freqs[(size_t)i++] = atof(l.data() + c + 1);
                }
            });
        }
        std::string la = rd("/proc/loadavg");
        sscanf(la.c_str(), "%lf %lf %lf", &load[0], &load[1], &load[2]);
        uptime = atof(rd("/proc/uptime").c_str());
        readTemps();
    }

    double freqAvg() const {
        double s = 0; int c = 0;
        for (double f : freqs) if (f > 0) { s += f; c++; }
        return c ? s / c : -1;
    }
};

struct Mem {
    std::unordered_map<std::string, long long> info;
    Ring hist, swapHist;

    void update(double) {
        std::string mi = rd("/proc/meminfo");
        eachLine(mi, [&](std::string_view l) {
            size_t c = l.find(':');
            if (c == std::string_view::npos) return;
            std::string k(l.substr(0, c));
            long long v = atoll(l.data() + c + 1);
            info[k] = v * 1024;
        });
        push(hist, pct());
        push(swapHist, swapPct());
    }
    long long g(const char* k, long long d = 0) const {
        auto it = info.find(k);
        return it == info.end() ? d : it->second;
    }
    long long total() const { return g("MemTotal"); }
    long long avail() const { return g("MemAvailable", g("MemFree")); }
    long long used()  const { return std::max(0LL, total() - avail()); }
    double pct() const { return total() ? used() * 100.0 / total() : 0.0; }
    long long swapTotal() const { return g("SwapTotal"); }
    long long swapUsed()  const { return std::max(0LL, swapTotal() - g("SwapFree")); }
    double swapPct() const { return swapTotal() ? swapUsed() * 100.0 / swapTotal() : 0.0; }
};

struct Mount {
    std::string dev, mp, fs;
    long long total = 0, used = 0, free_ = 0;
    long long inodes = 0, iused = 0;
    double pct = 0, ipct = 0;
};
struct DiskDev {
    Ring rd_, wr;
    double rrate = 0, wrate = 0, util = 0, iops = 0;
    long long pRd = 0, pWr = 0, pMs = 0, pIo = 0;
    bool seen = false;
};

struct Disk {
    std::vector<Mount> mounts;
    std::map<std::string, DiskDev> devs;
    Ring rdHist, wrHist;
    double rdRate = 0, wrRate = 0;
    double lastScan = -1e9;
    std::map<std::string, double> temps;

    static bool skipFs(const std::string& fs) {
        static const char* S[] = {"tmpfs","devtmpfs","squashfs","overlay","proc","sysfs",
            "cgroup","cgroup2","devpts","efivarfs","autofs","mqueue","hugetlbfs","debugfs",
            "tracefs","pstore","bpf","configfs","fusectl","securityfs","ramfs",
            "binfmt_misc","nsfs","fuse.gvfsd-fuse","fuse.portal","rpc_pipefs", nullptr};
        for (int i = 0; S[i]; i++) if (fs == S[i]) return true;
        return false;
    }
    // 只算整顆碟，不重複計分割區
    static bool isPartition(const std::string& n) {
        if (n.size() < 4) return false;
        if (!isdigit((unsigned char)n.back())) return false;
        if (startsWith(n, "nvme")) { size_t p = n.find('p'); return p != std::string::npos && p > 4; }
        if (startsWith(n, "mmcblk")) { size_t p = n.find('p'); return p != std::string::npos; }
        return startsWith(n, "sd") || startsWith(n, "vd") || startsWith(n, "hd");
    }
    static bool skipDev(const std::string& n) {
        return startsWith(n, "loop") || startsWith(n, "ram") || startsWith(n, "zram") ||
               startsWith(n, "dm-") || startsWith(n, "sr");
    }

    void scanMounts() {
        std::vector<Mount> out;
        std::vector<std::string> sigs;
        std::string mt = rd("/proc/mounts");
        eachLine(mt, [&](std::string_view l) {
            auto f = splitWS(std::string(l));
            if (f.size() < 3) return;
            std::string dev = f[0], mp = f[1], fs = f[2];
            size_t pos;
            while ((pos = mp.find("\\040")) != std::string::npos) mp.replace(pos, 4, " ");
            if (skipFs(fs) || !startsWith(dev, "/dev")) return;
            struct statvfs st;
            if (statvfs(mp.c_str(), &st) != 0 || st.f_blocks == 0) return;
            Mount m;
            m.dev = dev; m.mp = mp; m.fs = fs;
            m.total  = (long long)st.f_blocks * st.f_frsize;
            m.free_  = (long long)st.f_bavail * st.f_frsize;
            m.used   = m.total - (long long)st.f_bfree * st.f_frsize;
            m.inodes = (long long)st.f_files;
            m.iused  = (long long)(st.f_files - st.f_ffree);
            // df 的 Use% 定義：used/(used+avail)，不含保留給 root 的區塊
            long long den = m.used + m.free_;
            m.pct  = den ? m.used * 100.0 / den : 0.0;
            m.ipct = m.inodes ? m.iused * 100.0 / m.inodes : 0.0;
            // bind mount（例如 snap）統計完全相同，不重複列
            std::string sig = dev + "|" + std::to_string(m.total) + "|" + std::to_string(m.used);
            if (std::find(sigs.begin(), sigs.end(), sig) != sigs.end()) return;
            sigs.push_back(sig);
            out.push_back(std::move(m));
        });
        std::sort(out.begin(), out.end(),
                  [](const Mount& a, const Mount& b) { return a.total > b.total; });
        mounts = std::move(out);
        scanNvmeTemp();
    }

    void scanNvmeTemp() {
        temps.clear();
        DIR* d = opendir("/sys/class/hwmon");
        if (!d) return;
        while (dirent* e = readdir(d)) {
            if (e->d_name[0] == '.') continue;
            std::string p = std::string("/sys/class/hwmon/") + e->d_name;
            if (rdTrim(p + "/name") != "nvme") continue;
            long long v = rdLongOr(p + "/temp1_input", -1);
            if (v <= 0) continue;
            char buf[512];
            ssize_t n2 = readlink((p + "/device").c_str(), buf, sizeof buf - 1);
            std::string dev = "nvme";
            if (n2 > 0) {
                buf[n2] = 0;
                std::string s2(buf);
                size_t sl = s2.find_last_of('/');
                dev = (sl == std::string::npos) ? s2 : s2.substr(sl + 1);
            }
            temps[dev] = v / 1000.0;
        }
        closedir(d);
    }

    void update(double dt) {
        double t = nowMono();
        if (t - lastScan > 5.0) { scanMounts(); lastScan = t; }
        double rb = 0, wb = 0;
        std::string ds = rd("/proc/diskstats");
        eachLine(ds, [&](std::string_view l) {
            auto f = splitWS(std::string(l));
            if (f.size() < 14) return;
            const std::string& name = f[2];
            if (skipDev(name) || isPartition(name)) return;
            long long rs = atoll(f[5].c_str()) * 512;
            long long ws = atoll(f[9].c_str()) * 512;
            long long ms = atoll(f[12].c_str());
            long long io = atoll(f[11].c_str());
            DiskDev& d2 = devs[name];
            if (d2.seen && dt > 0) {
                d2.rrate = std::max(0.0, (rs - d2.pRd) / dt);
                d2.wrate = std::max(0.0, (ws - d2.pWr) / dt);
                d2.util  = clampv(std::max(0.0, (double)(ms - d2.pMs)) / (dt * 1000.0) * 100.0,
                                  0.0, 100.0);
                d2.iops  = std::max(0.0, (io - d2.pIo) / dt);
                push(d2.rd_, d2.rrate);
                push(d2.wr, d2.wrate);
                rb += d2.rrate; wb += d2.wrate;
            }
            d2.pRd = rs; d2.pWr = ws; d2.pMs = ms; d2.pIo = io; d2.seen = true;
        });
        rdRate = rb; wrRate = wb;
        push(rdHist, rb);
        push(wrHist, wb);
    }

    void modelOf(const std::string& n, std::string& model, std::string& kind, long long& size) {
        model = rdTrim("/sys/block/" + n + "/device/model");
        if (model.empty()) model = "?";
        kind = (rdLongOr("/sys/block/" + n + "/queue/rotational", 0) == 1) ? "HDD" : "SSD";
        long long sec = rdLongOr("/sys/block/" + n + "/size", 0);
        size = sec * 512;
    }
};

struct Iface {
    Ring rx, tx;
    double rxr = 0, txr = 0;
    long long rxb = 0, txb = 0, pRx = 0, pTx = 0;
    long long err = 0, drop = 0;
    bool up = false, skip = false, seen = false;
    long long speed = -1;
    std::string mac;
    std::vector<std::string> addrs;
};

struct Net {
    std::map<std::string, Iface> ifaces;
    Ring rxHist, txHist;
    double rx = 0, tx = 0;

    static bool skipName(const std::string& n) {
        return n == "lo" || startsWith(n, "veth") || startsWith(n, "br-") ||
               startsWith(n, "virbr") || startsWith(n, "docker") ||
               startsWith(n, "tun") || startsWith(n, "tap");
    }

    void update(double dt) {
        double rt = 0, tt = 0;
        std::string nd = rd("/proc/net/dev");
        int line = 0;
        eachLine(nd, [&](std::string_view l) {
            if (line++ < 2) return;
            size_t c = l.find(':');
            if (c == std::string_view::npos) return;
            std::string name(l.substr(0, c));
            size_t a = name.find_first_not_of(" \t");
            if (a == std::string::npos) return;
            name = name.substr(a);
            auto f = splitWS(std::string(l.substr(c + 1)));
            if (f.size() < 16) return;
            Iface& d2 = ifaces[name];
            d2.skip = skipName(name);
            long long rxb = atoll(f[0].c_str()), txb = atoll(f[8].c_str());
            if (d2.seen && dt > 0) {
                d2.rxr = std::max(0.0, (rxb - d2.pRx) / dt);
                d2.txr = std::max(0.0, (txb - d2.pTx) / dt);
                push(d2.rx, d2.rxr);
                push(d2.tx, d2.txr);
                if (!d2.skip) { rt += d2.rxr; tt += d2.txr; }
            }
            d2.pRx = rxb; d2.pTx = txb; d2.seen = true;
            d2.rxb = rxb; d2.txb = txb;
            d2.err  = atoll(f[2].c_str()) + atoll(f[10].c_str());
            d2.drop = atoll(f[3].c_str()) + atoll(f[11].c_str());
            d2.up = rdTrim("/sys/class/net/" + name + "/operstate") == "up";
            d2.speed = rdLongOr("/sys/class/net/" + name + "/speed", -1);
            d2.mac = rdTrim("/sys/class/net/" + name + "/address");
        });
        rx = rt; tx = tt;
        push(rxHist, rt);
        push(txHist, tt);
    }

    // 用 getifaddrs(3) 讀 IP —— 純 libc，不需要 fork 出一個 `ip` 行程
    void readAddrs() {
        for (auto& kv : ifaces) kv.second.addrs.clear();
        struct ifaddrs* head = nullptr;
        if (getifaddrs(&head) != 0) return;
        for (struct ifaddrs* a = head; a; a = a->ifa_next) {
            if (!a->ifa_addr || !a->ifa_name) continue;
            int fam = a->ifa_addr->sa_family;
            if (fam != AF_INET && fam != AF_INET6) continue;
            auto it = ifaces.find(a->ifa_name);
            if (it == ifaces.end()) continue;
            char host[NI_MAXHOST];
            socklen_t sl = (fam == AF_INET) ? sizeof(struct sockaddr_in)
                                            : sizeof(struct sockaddr_in6);
            if (getnameinfo(a->ifa_addr, sl, host, sizeof host, nullptr, 0,
                            NI_NUMERICHOST) != 0) continue;
            std::string ip = host;
            size_t pc = ip.find('%');              // 去掉 fe80::1%eth0 的 zone
            if (pc != std::string::npos) ip = ip.substr(0, pc);
            if (fam == AF_INET6 && startsWith(ip, "fe80")) continue;
            // 補上前綴長度，跟 `ip addr` 的顯示一致
            int bits = 0;
            if (a->ifa_netmask) {
                const unsigned char* m;
                int len;
                if (fam == AF_INET) {
                    m = (const unsigned char*)&((struct sockaddr_in*)a->ifa_netmask)->sin_addr;
                    len = 4;
                } else {
                    m = (const unsigned char*)&((struct sockaddr_in6*)a->ifa_netmask)->sin6_addr;
                    len = 16;
                }
                for (int i = 0; i < len; i++) bits += __builtin_popcount(m[i]);
            }
            it->second.addrs.push_back(bits ? ip + "/" + std::to_string(bits) : ip);
        }
        freeifaddrs(head);
    }
};

// ─────────────────────────────────────────────────────────────────────────────
// GPU：NVIDIA(nvidia-smi，跑在背景執行緒) + Intel i915 / AMD amdgpu (sysfs)
// ─────────────────────────────────────────────────────────────────────────────

struct NvCard {
    std::string idx, name, uuid, pstate, driver, gen, width, cmode;
    double util = 0, mutil = 0, temp = -1, power = -1, plimit = -1;
    double sclk = -1, mclk = -1, fan = -1;
    long long mused = 0, mtotal = 0;
};
struct NvProc { std::string pid, name, uuid; double mem = 0; };
struct IntCard {
    std::string card, path, vendor, name, kind, devid;
    long long freq = -1, fmax = -1, fmin = -1;
    double util = -1;            // <0 代表無法取得
    bool est = true;
    long long vramUsed = -1, vramTotal = -1;
};
struct DiagLine { int lvl; std::string text; };   // 0=info 1=ok 2=warn 3=bad

static bool haveCmd(const char* c) {
    std::string cmd = std::string("command -v ") + c + " >/dev/null 2>&1";
    return system(cmd.c_str()) == 0;
}
static std::string runCmd(const std::string& cmd, int* rcOut = nullptr) {
    FILE* p = popen(cmd.c_str(), "r");
    if (!p) { if (rcOut) *rcOut = -1; return ""; }
    std::string out;
    char buf[4096];
    size_t n;
    while ((n = fread(buf, 1, sizeof buf, p)) > 0) out.append(buf, n);
    int rc = pclose(p);
    if (rcOut) *rcOut = WIFEXITED(rc) ? WEXITSTATUS(rc) : -1;
    return out;
}
static std::vector<std::string> splitCSV(const std::string& l) {
    std::vector<std::string> v;
    size_t a = 0;
    while (true) {
        size_t c = l.find(',', a);
        std::string t = (c == std::string::npos) ? l.substr(a) : l.substr(a, c - a);
        size_t x = t.find_first_not_of(" \t\r");
        size_t y = t.find_last_not_of(" \t\r");
        v.push_back(x == std::string::npos ? "" : t.substr(x, y - x + 1));
        if (c == std::string::npos) break;
        a = c + 1;
    }
    return v;
}
static double toD(const std::string& s, double d = -1) {
    if (s.empty() || s == "N/A" || s == "[N/A]" || s == "[Not Supported]") return d;
    char* e = nullptr;
    double v = strtod(s.c_str(), &e);
    return e == s.c_str() ? d : v;
}

static const char* NVQ =
    "index,name,uuid,utilization.gpu,utilization.memory,memory.used,memory.total,"
    "temperature.gpu,power.draw,power.limit,clocks.sm,clocks.mem,fan.speed,pstate,"
    "driver_version,pcie.link.gen.current,pcie.link.width.current,compute_mode";

struct Gpu {
    std::vector<NvCard> nv;
    std::vector<NvProc> nvProcs;
    std::string nvError;
    bool nvPresent = false;
    std::map<std::string, Ring> hist;
    std::vector<IntCard> integrated;
    std::vector<DiagLine> diag;

    std::mutex mtx;
    std::atomic<bool> stopFlag{false};
    std::thread th;
    std::map<std::string, long long> rc6Prev;
    std::map<std::string, double> rc6T;
    std::atomic<bool> appsHot{false};   // GPU 頁開著時，運算行程清單要抓勤一點
    pid_t nvPid = -1;            // 常駐的 nvidia-smi -l
    int   nvFd  = -1;
    int   nvCount = 0;           // 有幾張 NVIDIA 卡（決定一「幀」有幾行）

    void start() {
        nvPresent = haveCmd("nvidia-smi");
        scanIntegrated();
        if (!nvPresent) return;
        // 先打一次確認驅動活著、順便知道有幾張卡；失敗的話錯誤訊息也從這裡拿
        pollNv();
        {
            std::lock_guard<std::mutex> lk(mtx);
            nvCount = (int)nv.size();
        }
        th = std::thread([this] { nvLoop(); });
    }
    void stop() {
        stopFlag.store(true);
        killChild();
        if (th.joinable()) th.join();
    }
    void killChild() {
        if (nvPid > 0) { kill(nvPid, SIGTERM); waitpid(nvPid, nullptr, 0); nvPid = -1; }
        if (nvFd >= 0) { close(nvFd); nvFd = -1; }
    }

    // 開一個常駐的 nvidia-smi -l，避免每次取樣都重新初始化驅動（那才是真正的開銷大頭）
    bool spawnNvLoop(int seconds) {
        int fds[2];
        if (pipe(fds) != 0) return false;
        pid_t pid = fork();
        if (pid < 0) { close(fds[0]); close(fds[1]); return false; }
        if (pid == 0) {
            close(fds[0]);
            dup2(fds[1], STDOUT_FILENO);
            dup2(fds[1], STDERR_FILENO);
            close(fds[1]);
            setpgid(0, 0);
            std::string q = std::string("--query-gpu=") + NVQ;
            std::string l = "-l" + std::to_string(seconds);
            execlp("nvidia-smi", "nvidia-smi", q.c_str(),
                   "--format=csv,noheader,nounits", l.c_str(), (char*)nullptr);
            _exit(127);
        }
        close(fds[1]);
        fcntl(fds[0], F_SETFD, FD_CLOEXEC);
        nvPid = pid;
        nvFd  = fds[0];
        return true;
    }

    void nvLoop() {
        std::string buf;
        std::vector<NvCard> frame;
        double lastApps = 0;
        while (!stopFlag.load()) {
            if (nvFd < 0 && !spawnNvLoop(2)) {
                for (int i = 0; i < 20 && !stopFlag.load(); i++)
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                continue;
            }
            struct pollfd pfd{nvFd, POLLIN, 0};
            int pr = poll(&pfd, 1, 300);
            if (stopFlag.load()) break;
            if (pr > 0 && (pfd.revents & (POLLIN | POLLHUP))) {
                char rb[4096];
                ssize_t n = read(nvFd, rb, sizeof rb);
                if (n <= 0) {                       // 子行程掛了：記錄原因並重來
                    killChild();
                    pollNv();
                    for (int i = 0; i < 30 && !stopFlag.load(); i++)
                        std::this_thread::sleep_for(std::chrono::milliseconds(100));
                    continue;
                }
                buf.append(rb, (size_t)n);
                size_t nl;
                while ((nl = buf.find('\n')) != std::string::npos) {
                    std::string line = buf.substr(0, nl);
                    buf.erase(0, nl + 1);
                    if (line.empty()) continue;
                    NvCard c;
                    if (!parseNvLine(line, c)) {     // 不是資料列 → 當成錯誤訊息
                        std::lock_guard<std::mutex> lk(mtx);
                        nvError = line;
                        continue;
                    }
                    // index 回到 0 代表新的一幀開始
                    if (!frame.empty() && c.idx == frame[0].idx) {
                        publish(frame);
                        frame.clear();
                    }
                    frame.push_back(std::move(c));
                    if (nvCount > 0 && (int)frame.size() >= nvCount) {
                        publish(frame);
                        frame.clear();
                    }
                }
            }
            // 查運算行程要另外 fork 一次 nvidia-smi，是這裡最貴的動作。
            // GPU 真的在跑東西、或使用者正開著 GPU 頁時才抓勤一點，否則 30 秒一次。
            bool busy = appsHot.load();
            if (!busy) {
                std::lock_guard<std::mutex> lk(mtx);
                for (auto& g : nv)
                    if (g.util > 1.0 || g.mused > 128LL * 1048576) { busy = true; break; }
            }
            double now = nowMono();
            if (now - lastApps > (busy ? 5.0 : 30.0)) { lastApps = now; pollApps(); }
        }
    }

    void publish(const std::vector<NvCard>& frame) {
        if (frame.empty()) return;
        std::lock_guard<std::mutex> lk(mtx);
        nv = frame;
        nvCount = (int)frame.size();
        nvError.clear();
    }

    bool parseNvLine(const std::string& line, NvCard& c) {
        auto v = splitCSV(line);
        if (v.size() < 18) return false;
        if (v[0].empty() || !isdigit((unsigned char)v[0][0])) return false;
        c.idx = v[0]; c.name = v[1]; c.uuid = v[2];
        c.util = toD(v[3], 0); c.mutil = toD(v[4], 0);
        c.mused = (long long)(toD(v[5], 0) * 1048576.0);
        c.mtotal = (long long)(toD(v[6], 0) * 1048576.0);
        c.temp = toD(v[7]); c.power = toD(v[8]); c.plimit = toD(v[9]);
        c.sclk = toD(v[10]); c.mclk = toD(v[11]); c.fan = toD(v[12]);
        c.pstate = v[13]; c.driver = v[14]; c.gen = v[15];
        c.width = v[16]; c.cmode = v[17];
        return true;
    }

    void pollApps() {
        int rc = 0;
        std::string po = runCmd("nvidia-smi --query-compute-apps=pid,process_name,used_memory,"
                                "gpu_uuid --format=csv,noheader,nounits 2>/dev/null", &rc);
        if (rc != 0) return;
        std::vector<NvProc> procs;
        eachLine(po, [&](std::string_view l) {
            if (l.empty()) return;
            auto v = splitCSV(std::string(l));
            if (v.size() < 4) return;
            NvProc p;
            p.pid = v[0]; p.name = v[1];
            p.mem = toD(v[2], 0) * 1048576.0;
            p.uuid = v[3];
            procs.push_back(std::move(p));
        });
        std::lock_guard<std::mutex> lk(mtx);
        nvProcs = std::move(procs);
    }

    void pollNv() {
        int rc = 0;
        std::string out = runCmd(std::string("nvidia-smi --query-gpu=") + NVQ +
                                " --format=csv,noheader,nounits 2>&1", &rc);
        if (rc != 0 || out.empty()) {
            std::string msg = out;
            size_t nlp = msg.find('\n');
            if (nlp != std::string::npos) msg = msg.substr(0, nlp);
            if (msg.empty()) msg = "nvidia-smi 失敗";
            std::lock_guard<std::mutex> lk(mtx);
            nvError = msg;
            nv.clear();
            return;
        }
        std::vector<NvCard> cards;
        eachLine(out, [&](std::string_view l) {
            if (l.empty()) return;
            NvCard c;
            if (parseNvLine(std::string(l), c)) cards.push_back(std::move(c));
        });
        if (cards.empty()) {
            std::string msg = out.substr(0, out.find('\n'));
            if (msg.empty()) msg = "nvidia-smi 無輸出";
            std::lock_guard<std::mutex> lk(mtx);
            nvError = msg;
            nv.clear();
            return;
        }
        pollApps();
        std::lock_guard<std::mutex> lk(mtx);
        nv = std::move(cards);
        nvError.clear();
    }

    void scanIntegrated() {
        integrated.clear();
        DIR* d = opendir("/sys/class/drm");
        if (!d) return;
        std::vector<std::string> cards;
        while (dirent* e = readdir(d)) {
            std::string n = e->d_name;
            if (n.size() < 5 || !startsWith(n, "card")) continue;
            bool alldig = true;
            for (size_t i = 4; i < n.size(); i++) if (!isdigit((unsigned char)n[i])) alldig = false;
            if (alldig) cards.push_back(n);
        }
        closedir(d);
        std::sort(cards.begin(), cards.end());
        for (auto& c : cards) {
            IntCard ic;
            ic.card = c;
            ic.path = "/sys/class/drm/" + c;
            std::string vend = rdTrim(ic.path + "/device/vendor");
            ic.devid = rdTrim(ic.path + "/device/device");
            if (vend == "0x8086") { ic.vendor = "Intel"; ic.kind = "i915"; ic.name = intelName(ic.devid); }
            else if (vend == "0x1002") { ic.vendor = "AMD"; ic.kind = "amdgpu"; ic.name = "AMD GPU " + ic.devid; }
            else if (vend == "0x10de") { ic.vendor = "NVIDIA"; ic.kind = "drm"; ic.name = "NVIDIA GPU " + ic.devid; }
            else continue;
            integrated.push_back(std::move(ic));
        }
    }

    static std::string intelName(const std::string& id) {
        static const std::map<std::string, std::string> M = {
            {"0xa780", "UHD Graphics 770 (Raptor Lake-S)"},
            {"0xa788", "UHD Graphics 730 (Raptor Lake-S)"},
            {"0x4680", "UHD Graphics 770 (Alder Lake-S)"},
            {"0x4692", "UHD Graphics 730 (Alder Lake-S)"},
            {"0x9a49", "Iris Xe Graphics (Tiger Lake)"},
            {"0x46a6", "Iris Xe Graphics (Alder Lake-P)"},
            {"0x3e92", "UHD Graphics 630 (Coffee Lake)"},
        };
        auto it = M.find(id);
        return it == M.end() ? ("Intel Graphics " + id) : it->second;
    }

    void updateIntegrated() {
        double now = nowMono();
        for (auto& c : integrated) {
            long long a = rdLongOr(c.path + "/gt_act_freq_mhz", -1);
            c.freq = (a >= 0) ? a : rdLongOr(c.path + "/gt_cur_freq_mhz", -1);
            c.fmax = rdLongOr(c.path + "/gt_max_freq_mhz", -1);
            if (c.fmax < 0) c.fmax = rdLongOr(c.path + "/gt_RP0_freq_mhz", -1);
            c.fmin = rdLongOr(c.path + "/gt_min_freq_mhz", -1);
            if (c.fmin < 0) c.fmin = rdLongOr(c.path + "/gt_RPn_freq_mhz", -1);
            double util = -1;
            if (c.kind == "amdgpu") {
                long long b = rdLongOr(c.path + "/device/gpu_busy_percent", -1);
                if (b >= 0) util = (double)b;
                c.vramTotal = rdLongOr(c.path + "/device/mem_info_vram_total", -1);
                c.vramUsed  = rdLongOr(c.path + "/device/mem_info_vram_used", -1);
                c.est = false;
            } else {
                // i915 沒有 busy% 介面。改用 RC6（GPU 省電休眠）殘留時間反推：
                // 這段時間內「沒在睡」的比例，就約等於忙碌比例。
                long long rc6 = rdLongOr(c.path + "/power/rc6_residency_ms", -1);
                if (rc6 >= 0) {
                    auto ip = rc6Prev.find(c.card);
                    auto it = rc6T.find(c.card);
                    if (ip == rc6Prev.end() || it == rc6T.end()) {
                        rc6Prev[c.card] = rc6; rc6T[c.card] = now;
                    } else {
                        double el = now - it->second;
                        if (el >= 0.05) {
                            util = clampv(100.0 - (rc6 - ip->second) / (el * 1000.0) * 100.0,
                                          0.0, 100.0);
                            rc6Prev[c.card] = rc6; rc6T[c.card] = now;
                        } else {
                            util = c.util;      // 間隔太短，沿用上一次
                        }
                    }
                }
                c.est = true;
            }
            c.util = util;
            push(hist[c.card], util < 0 ? 0.0 : util);
        }
    }

    void update(double) {
        updateIntegrated();
        std::lock_guard<std::mutex> lk(mtx);
        for (auto& g : nv) push(hist["nv" + g.idx], g.util);
    }

    void snapshot(std::vector<NvCard>& c, std::vector<NvProc>& p, std::string& e) {
        {
            std::lock_guard<std::mutex> lk(mtx);
            c = nv; p = nvProcs; e = nvError;
        }
        // 診斷很貴（要跑 dpkg-query / dkms / mokutil），只有真的壞掉時才查一次
        if (!e.empty() && diag.empty()) diagnose();
    }

    // nvidia-smi 掛掉時，把「為什麼」查出來
    void diagnose() {
        diag.clear();
        bool hasPci = false;
        std::string lspci = runCmd("lspci 2>/dev/null");
        eachLine(lspci, [&](std::string_view l) {
            std::string s(l);
            if (s.find("NVIDIA") == std::string::npos) return;
            if (s.find("VGA") == std::string::npos && s.find("3D controller") == std::string::npos) return;
            hasPci = true;
            size_t c = s.rfind(':');
            diag.push_back({0, "PCI 上偵測到 NVIDIA:" +
                               (c == std::string::npos ? s : s.substr(c + 1))});
        });
        if (!hasPci) return;
        std::string mods = rd("/proc/modules");
        bool loaded = startsWith(mods, "nvidia ") || mods.find("\nnvidia ") != std::string::npos;
        diag.push_back({loaded ? 1 : 3,
                        std::string("kernel module `nvidia` ") + (loaded ? "已載入" : "未載入")});
        std::string devs;
        if (DIR* d = opendir("/dev")) {
            std::vector<std::string> v;
            while (dirent* e = readdir(d)) if (startsWith(e->d_name, "nvidia")) v.emplace_back(e->d_name);
            closedir(d);
            std::sort(v.begin(), v.end());
            for (size_t i = 0; i < v.size() && i < 6; i++) devs += (i ? ", " : "") + v[i];
        }
        diag.push_back({devs.empty() ? 3 : 1,
                        "/dev/nvidia* 裝置節點: " + (devs.empty() ? "不存在" : devs)});
        std::string pk = runCmd("dpkg-query -W -f='${Package} ${Version}\\n' 'nvidia-dkms-*' "
                                "'nvidia-driver-*' 2>/dev/null");
        std::vector<std::string> pkgs;
        eachLine(pk, [&](std::string_view l) {
            auto f = splitWS(std::string(l));
            if (f.size() >= 2) pkgs.push_back(std::string(l));
        });
        if (!pkgs.empty()) {
            std::string j;
            for (size_t i = 0; i < pkgs.size() && i < 6; i++) j += (i ? "; " : "") + pkgs[i];
            diag.push_back({0, "已安裝套件: " + j});
            std::vector<std::string> vers;
            for (auto& p : pkgs) {
                size_t k = p.find("nvidia-dkms-");
                if (k == std::string::npos) continue;
                std::string v;
                for (size_t i = k + 12; i < p.size() && isdigit((unsigned char)p[i]); i++) v += p[i];
                if (!v.empty() && std::find(vers.begin(), vers.end(), v) == vers.end()) vers.push_back(v);
            }
            if (vers.size() > 1) {
                std::sort(vers.begin(), vers.end());
                std::string j2;
                for (size_t i = 0; i < vers.size(); i++) j2 += (i ? ", " : "") + vers[i];
                diag.push_back({3, "同時裝了多個 nvidia-dkms 版本 (" + j2 + ") — 會互相打架"});
            }
        }
        struct utsname un; uname(&un);
        std::string curK = un.release;
        int rc = 0;
        std::string dk = runCmd("dkms status 2>/dev/null", &rc);
        if (rc == 0) {
            if (dk.find_first_not_of(" \t\r\n") != std::string::npos) {
                std::vector<std::string> ks;
                int shown = 0;
                eachLine(dk, [&](std::string_view l) {
                    if (l.empty()) return;
                    if (shown < 4) { diag.push_back({0, "dkms: " + std::string(l)}); shown++; }
                    // 抓出 ", <kernel>, x86_64" 中間那段
                    std::string s(l);
                    size_t a = s.find(", ");
                    if (a == std::string::npos) return;
                    size_t b = s.find(", ", a + 2);
                    if (b == std::string::npos) return;
                    std::string k = s.substr(a + 2, b - a - 2);
                    if (!k.empty() && std::find(ks.begin(), ks.end(), k) == ks.end()) ks.push_back(k);
                });
                if (!ks.empty() && std::find(ks.begin(), ks.end(), curK) == ks.end()) {
                    std::string j;
                    for (size_t i = 0; i < ks.size(); i++) j += (i ? "/" : "") + ks[i];
                    diag.push_back({3, "模組只替 " + j + " 編過，但目前跑的是 " + curK +
                                       " — 核心升級後沒重建"});
                }
            } else {
                diag.push_back({3, "dkms status 是空的 — 沒有任何模組替目前核心編譯過"});
            }
        }
        diag.push_back({0, "目前核心: " + curK});
        int rc3 = 0;
        std::string sb = runCmd("mokutil --sb-state 2>/dev/null", &rc3);
        if (rc3 == 0 && !sb.empty()) {
            std::string first = sb.substr(0, sb.find('\n'));
            bool on = first.find("enabled") != std::string::npos;
            diag.push_back({on ? 2 : 0, "Secure Boot: " + first});
        }
    }
};

// ─────────────────────────────────────────────────────────────────────────────
// 行程
// ─────────────────────────────────────────────────────────────────────────────

struct ProcInfo {
    int pid = 0, ppid = 0, thr = 0, nice = 0;
    char state = '?';
    double cpu = 0, cputime = 0;
    long long rss = 0, vsz = 0;
    uid_t uid = 0;
    std::string name, user, cmd;
};

struct Procs {
    std::vector<ProcInfo> list;
    std::unordered_map<int, long long> prevJif;
    std::unordered_map<uid_t, std::string> userCache;
    int total = 0, threads = 0;
    std::map<char, int> byState;

    const std::string& userName(uid_t u) {
        auto it = userCache.find(u);
        if (it != userCache.end()) return it->second;
        struct passwd* pw = getpwuid(u);
        return userCache[u] = pw ? pw->pw_name : std::to_string(u);
    }

    void update(double dt, int ncpu) {
        std::vector<ProcInfo> out;
        out.reserve(list.empty() ? 512 : list.size() + 32);
        std::unordered_map<int, long long> cur;
        cur.reserve(prevJif.size() ? prevJif.size() * 2 : 1024);
        threads = 0;
        byState.clear();

        DIR* d = opendir("/proc");
        if (!d) return;
        std::string path, st;
        char buf[4096];
        while (dirent* e = readdir(d)) {
            const char* nm = e->d_name;
            if (!isdigit((unsigned char)nm[0])) continue;
            int pid = atoi(nm);
            path = "/proc/"; path += nm; path += "/stat";
            int fd = open(path.c_str(), O_RDONLY | O_CLOEXEC);
            if (fd < 0) continue;
            ssize_t n = read(fd, buf, sizeof buf - 1);
            struct stat sb;
            bool okStat = (fstat(fd, &sb) == 0);
            close(fd);
            if (n <= 0) continue;
            buf[n] = 0;

            // comm 可能含空白與括號，所以要從最後一個 ')' 切
            char* rp = strrchr(buf, ')');
            char* lp = strchr(buf, '(');
            if (!rp || !lp || rp < lp) continue;
            ProcInfo p;
            p.pid = pid;
            p.name.assign(lp + 1, (size_t)(rp - lp - 1));
            auto f = splitWS(rp + 2);
            if (f.size() < 22) continue;
            p.state = f[0].empty() ? '?' : f[0][0];
            p.ppid  = atoi(f[1].c_str());
            long long ut = atoll(f[11].c_str()), stt = atoll(f[12].c_str());
            p.nice  = atoi(f[16].c_str());
            p.thr   = atoi(f[17].c_str());
            p.vsz   = atoll(f[20].c_str());
            p.rss   = atoll(f[21].c_str()) * PAGESZ_;
            long long jif = ut + stt;
            cur[pid] = jif;
            auto pv = prevJif.find(pid);
            if (pv != prevJif.end() && dt > 0)
                p.cpu = clampv((jif - pv->second) / (double)CLK_TCK_ / dt * 100.0,
                               0.0, 100.0 * ncpu);
            p.cputime = jif / (double)CLK_TCK_;
            p.uid = okStat ? sb.st_uid : 0;
            p.user = userName(p.uid);

            path = "/proc/"; path += nm; path += "/cmdline";
            int cfd = open(path.c_str(), O_RDONLY | O_CLOEXEC);
            if (cfd >= 0) {
                ssize_t cn = read(cfd, buf, sizeof buf - 1);
                close(cfd);
                if (cn > 0) {
                    for (ssize_t i = 0; i < cn; i++) if (buf[i] == '\0') buf[i] = ' ';
                    buf[cn] = 0;
                    p.cmd = buf;
                    while (!p.cmd.empty() && p.cmd.back() == ' ') p.cmd.pop_back();
                }
            }
            if (p.cmd.empty()) p.cmd = "[" + p.name + "]";
            threads += p.thr;
            byState[p.state]++;
            out.push_back(std::move(p));
        }
        closedir(d);
        prevJif.swap(cur);
        list.swap(out);
        total = (int)list.size();
    }
};

// ─────────────────────────────────────────────────────────────────────────────
// 應用程式主體
// ─────────────────────────────────────────────────────────────────────────────

enum View { V_OVER, V_CPU, V_MEM, V_GPU, V_DISK, V_NET, V_PROC, V_COUNT };
static const char* VIEW_TITLE[V_COUNT] =
    {"總覽", "處理器", "記憶體", "顯示卡", "儲存", "網路", "行程"};
static const char* VIEW_KEY[V_COUNT] = {"~", "1", "2", "3", "4", "5", "6"};

struct App {
    Screen S;
    double interval = 1.0;
    View view = V_OVER;
    bool paused = false, showHelp = false, editing = false;
    int scroll = 0;
    int sortKey = 0;              // 0=cpu 1=rss 2=pid 3=thr
    std::string filter, msg;
    double msgT = -1e9;

    Cpu cpu; Mem mem; Disk disk; Net net; Gpu gpu; Procs proc;
    double last = 0, procLast = 0;
    long ticks = 0;
    std::string host, osname, kernel, me;

    App() {
        char hn[256] = {0};
        gethostname(hn, sizeof hn - 1);
        host = hn;
        struct utsname un; uname(&un);
        kernel = un.release;
        osname = un.sysname;
        std::string osr = rd("/etc/os-release");
        eachLine(osr, [&](std::string_view l) {
            if (startsWith(l, "PRETTY_NAME=")) {
                std::string v(l.substr(12));
                if (v.size() >= 2 && v.front() == '"') v = v.substr(1, v.size() - 2);
                osname = v;
            }
        });
        struct passwd* pw = getpwuid(getuid());
        me = pw ? pw->pw_name : "?";
        gpu.start();
        sample(0.0);
        net.readAddrs();
        last = nowMono();
        procLast = nowMono();
    }
    ~App() { gpu.stop(); }

    void note(const std::string& s) { msg = s; msgT = nowMono(); }

    void sample(double dt) {
        cpu.update(dt);
        mem.update(dt);
        disk.update(dt);
        net.update(dt);
        gpu.update(dt);
        // 掃全部 /proc/<pid> 是單次取樣裡最貴的一段，而行程清單本來就不需要
        // 跟圖表一樣快。這裡獨立節流：最快 0.75 秒一次。
        double now = nowMono();
        double pdt = now - procLast;
        if (pdt >= std::max(interval, 0.75) - 1e-3 || ticks == 0) {
            proc.update(ticks == 0 ? 0.0 : pdt, std::max(1, cpu.n));
            procLast = now;
        }
        ticks++;
        if (ticks % 15 == 0) net.readAddrs();
    }

    // 依排序鍵取前 n 名
    std::vector<const ProcInfo*> topProcs(int n, bool useFilter = false) {
        std::vector<const ProcInfo*> v;
        v.reserve(proc.list.size());
        std::string f = filter;
        for (auto& c : f) c = (char)tolower((unsigned char)c);
        for (auto& p : proc.list) {
            if (useFilter && !f.empty()) {
                std::string nm = p.name, cm = p.cmd, us = p.user;
                for (auto& c : nm) c = (char)tolower((unsigned char)c);
                for (auto& c : cm) c = (char)tolower((unsigned char)c);
                for (auto& c : us) c = (char)tolower((unsigned char)c);
                if (nm.find(f) == std::string::npos && cm.find(f) == std::string::npos &&
                    us.find(f) == std::string::npos && std::to_string(p.pid) != f) continue;
            }
            v.push_back(&p);
        }
        int k = sortKey;
        auto cmpf = [k](const ProcInfo* a, const ProcInfo* b) {
            switch (k) {
                case 1: return a->rss > b->rss;
                case 2: return a->pid > b->pid;
                case 3: return a->thr > b->thr;
                default: return a->cpu > b->cpu;
            }
        };
        if (n > 0 && (int)v.size() > n) {
            std::partial_sort(v.begin(), v.begin() + n, v.end(), cmpf);
            v.resize((size_t)n);
        } else {
            std::sort(v.begin(), v.end(), cmpf);
        }
        return v;
    }

    // ── 頁首 / 頁尾 ─────────────────────────────────────────────────────
    void header(int w) {
        int x = 0;
        S.put(0, x, "▎", C_ACCENT, A_BOLD); x += 1;
        S.put(0, x, "sysview ", C_WHITE, A_BOLD); x += 8;
        S.put(0, x, host, C_ACCENT, A_BOLD); x += dw(host) + 2;
        std::string segs[3] = {osname, "· " + kernel, "up " + durs(cpu.uptime)};
        Col scol[3] = {C_DIM, C_FAINT, C_DIM};
        for (int i = 0; i < 3; i++) {
            if (x + dw(segs[i]) + 2 > w - 36) break;
            S.put(0, x, segs[i], scol[i]); x += dw(segs[i]) + 2;
        }
        std::string right = fmt("load %.2f %.2f %.2f", cpu.load[0], cpu.load[1], cpu.load[2]);
        time_t t = time(nullptr);
        struct tm tmv; localtime_r(&t, &tmv);
        char cb[32]; strftime(cb, sizeof cb, "%H:%M:%S", &tmv);
        std::string clock = cb;
        int rx = w - dw(clock) - 1;
        S.put(0, rx, clock, C_WHITE, A_BOLD);
        rx -= dw(right) + 3;
        S.put(0, rx, right, heat(cpu.load[0] / std::max(1, cpu.n) * 100), A_BOLD);

        std::string rule;
        for (int i = 0; i < w; i++) rule += "─";
        S.put(1, 0, rule, C_FRAME);
        int tx = 2;
        for (int v = 0; v < V_COUNT; v++) {
            std::string label = fmt(" %s %s ", VIEW_KEY[v], VIEW_TITLE[v]);
            int lw = dw(label);
            if (tx + lw + 2 > w - 2) break;
            if (v == view) {
                S.put(1, tx, "┤", C_FRAME);
                S.put(1, tx + 1, label, C_ACCENT, A_BOLD | A_REV);
                S.put(1, tx + 1 + lw, "├", C_FRAME);
            } else {
                S.put(1, tx + 1, label, C_DIM);
            }
            tx += lw + 2;
        }
    }

    void footer(int h, int w) {
        int y = h - 1;
        S.put(y, 0, std::string((size_t)std::max(0, w - 1), ' '), C_FRAME);
        if (editing) {
            S.put(y, 1, trunc("篩選 (Enter 確定, Esc 取消): " + filter + "█", w - 2),
                  C_WARN, A_BOLD);
            return;
        }
        if (!msg.empty() && nowMono() - msgT < 3.0) {
            S.put(y, 1, trunc(msg, w - 2), C_WARN, A_BOLD);
            return;
        }
        std::vector<std::pair<std::string, std::string>> keys;
        if (view == V_PROC) {
            static const char* SN[4] = {"cpu", "rss", "pid", "thr"};
            keys = {{"s", std::string("排序:") + SN[sortKey]}, {"/", "篩選"},
                    {"↑↓", "捲動"}, {"~", "總覽"}, {"?", "說明"}, {"q", "離開"}};
        } else {
            keys = {{"1-6", "深入"}, {"~", "總覽"}, {"Tab", "切換"}, {"+/-", "更新率"},
                    {"空白", "暫停"}, {"?", "說明"}, {"q", "離開"}};
        }
        int x = 1;
        for (auto& kv : keys) {
            if (x + dw(kv.first) + dw(kv.second) + 3 > w - 22) break;
            S.put(y, x, kv.first, C_ACCENT, A_BOLD); x += dw(kv.first) + 1;
            S.put(y, x, kv.second, C_DIM);           x += dw(kv.second) + 2;
        }
        std::string st = fmt("%s %.1fs", paused ? "‖ 暫停" : "▶", interval);
        S.put(y, w - dw(st) - 2, st, paused ? C_WARN : C_FAINT, paused ? A_BOLD : 0);
    }

    void hint(int y, int x, int w, const std::string& t) {
        S.put(y, x, trunc("💡 " + t, w), C_FAINT);
    }

    // ── 總覽面板 ────────────────────────────────────────────────────────
    int coreCellW(int iw) { return iw >= 54 ? 9 : 7; }
    int coreRows(int iw) {
        int per = std::max(1, iw / coreCellW(iw));
        return cpu.n ? (cpu.n + per - 1) / per : 0;
    }
    void coreGrid(int y, int x, int h, int w) {
        if (h <= 0 || cpu.n == 0) return;
        int cw = coreCellW(w), per = std::max(1, w / cw), bw = cw - 4;
        for (int i = 0; i < cpu.n; i++) {
            int r = i / per, c = i % per;
            if (r >= h) { S.put(y + h - 1, x + w - 3, "…", C_FAINT); break; }
            S.put(y + r, x + c * cw, fmt("%2d", i), C_FAINT);
            bar(S, y + r, x + c * cw + 3, bw, cpu.corePct[(size_t)i]);
        }
    }

    void pCpu(int y, int x, int h, int w) {
        frameBox(S, y, x, h, w, "CPU", C_FRAME, C_CPU, fmt("%d 執行緒", cpu.n));
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2;
        if (ih < 1) return;
        S.put(iy, ix, fmt("%5.1f%%", cpu.pct), heat(cpu.pct), A_BOLD);
        std::string info;
        if (cpu.freqAvg() > 0) info += hzs(cpu.freqAvg());
        if (cpu.temp >= 0) info += (info.empty() ? "" : "  ") + fmt("%.0f°C", cpu.temp);
        info += (info.empty() ? "" : "  ") + fmt("load %.2f", cpu.load[0]);
        if (cpu.iowait > 1) info += fmt("  iowait %.0f%%", cpu.iowait);
        S.put(iy, ix + 7, info, C_DIM);
        std::string model = trunc(cpu.model, std::max(0, iw - 9 - dw(info) - 8));
        if (dw(model) > 12) S.put(iy, ix + iw - dw(model), model, C_FAINT);
        int gh = std::max(1, std::min(ih - 1 - coreRows(iw), ih - 2));
        int cy;
        if (gh >= 2) { drawGraph(S, iy + 1, ix, iw, gh, cpu.total, C_CPU, 100); cy = iy + 1 + gh; }
        else cy = iy + 1;
        coreGrid(cy, ix, ih - (cy - iy), iw);
    }

    void pMem(int y, int x, int h, int w) {
        frameBox(S, y, x, h, w, "記憶體", C_FRAME, C_MEM, human((double)mem.total()));
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2;
        if (ih < 2) return;
        S.put(iy, ix, "RAM", C_MEM, A_BOLD);
        std::string txt = human((double)mem.used()) + " / " + human((double)mem.total());
        int bw = std::max(4, iw - 4 - dw(txt) - 8);
        bar(S, iy, ix + 4, bw, mem.pct());
        S.put(iy, ix + 4 + bw + 1, fmt("%5.1f%%", mem.pct()), heat(mem.pct()), A_BOLD);
        S.put(iy, ix + iw - dw(txt), txt, C_DIM);
        int row = iy + 1;
        if (mem.swapTotal()) {
            S.put(row, ix, "SWP", C_MEM);
            std::string t2 = human((double)mem.swapUsed()) + " / " + human((double)mem.swapTotal());
            int b2 = std::max(4, iw - 4 - dw(t2) - 8);
            bar(S, row, ix + 4, b2, mem.swapPct());
            S.put(row, ix + 4 + b2 + 1, fmt("%5.1f%%", mem.swapPct()), heat(mem.swapPct()));
            S.put(row, ix + iw - dw(t2), t2, C_DIM);
            row++;
        }
        int gh = ih - (row - iy) - 1;
        if (gh >= 2) { drawGraph(S, row, ix, iw, gh, mem.hist, C_MEM, 100); row += gh; }
        if (row - iy < ih) {
            S.put(row, ix, trunc("快取 " + human((double)mem.g("Cached")) +
                                 "   緩衝 " + human((double)mem.g("Buffers")) +
                                 "   可用 " + human((double)mem.avail()), iw), C_FAINT);
        }
    }

    // 畫一張卡的兩行摘要，回傳下一列的 y
    int gpuRow(int row, int ix, int iw, const std::string& name, double util, bool est,
               double temp, double power, double sclk,
               long long mused, long long mtotal, int avail) {
        S.put(row, ix, trunc(name, std::max(10, iw - 30)), C_GPU, A_BOLD);
        std::string bits;
        if (temp >= 0)  bits += fmt("%.0f°C", temp);
        if (power >= 0) bits += (bits.empty() ? "" : "  ") + fmt("%.0fW", power);
        if (sclk >= 0)  bits += (bits.empty() ? "" : "  ") + hzs(sclk);
        if (!bits.empty()) S.put(row, ix + iw - dw(bits), bits, C_DIM);
        row++;
        if (avail < 2) return row;
        S.put(row, ix, est ? "使用~" : "使用", C_FAINT);
        int bw = std::max(6, iw - 22);
        bar(S, row, ix + 5, bw, util < 0 ? 0 : util);
        S.put(row, ix + 5 + bw + 1, util < 0 ? "  n/a" : fmt("%5.1f%%", util),
              util < 0 ? C_FAINT : heat(util), util < 0 ? 0 : A_BOLD);
        row++;
        if (mtotal > 0 && avail >= 3) {
            double mp = mused * 100.0 / mtotal;
            S.put(row, ix, "VRAM", C_FAINT);
            std::string t2 = human((double)mused) + "/" + human((double)mtotal);
            int b2 = std::max(6, iw - 8 - dw(t2));
            bar(S, row, ix + 5, b2, mp);
            S.put(row, ix + 5 + b2 + 1, t2, C_DIM);
            row++;
        }
        return row;
    }

    void pGpu(int y, int x, int h, int w) {
        std::vector<NvCard> cards; std::vector<NvProc> gprocs; std::string err;
        gpu.snapshot(cards, gprocs, err);
        int nInt = 0;
        for (auto& c : gpu.integrated) if (!(c.vendor == "NVIDIA" && !cards.empty())) nInt++;
        int tot = (int)cards.size() + nInt;
        frameBox(S, y, x, h, w, "GPU", C_FRAME, C_GPU, tot ? fmt("%d 張", tot) : "無");
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2, row = iy;
        if (!err.empty() && gpu.nvPresent) {
            S.put(row, ix, trunc("✗ NVIDIA: " + err, iw), C_CRIT, A_BOLD);
            row++;
            if (row - iy < ih) { S.put(row, ix, trunc("按 3 看完整診斷與修復建議", iw), C_WARN); row++; }
        }
        for (auto& g : cards) {
            if (ih - (row - iy) < 2) break;
            row = gpuRow(row, ix, iw, g.name, g.util, false, g.temp, g.power, g.sclk,
                         g.mused, g.mtotal, ih - (row - iy));
        }
        for (auto& c : gpu.integrated) {
            if (c.vendor == "NVIDIA" && !cards.empty()) continue;
            if (ih - (row - iy) < 2) break;
            row = gpuRow(row, ix, iw, c.name, c.util, c.est, -1, -1, (double)c.freq,
                         c.vramUsed, c.vramTotal, ih - (row - iy));
        }
        int left = ih - (row - iy);
        if (left >= 2 && tot) {
            std::string key = !cards.empty() ? ("nv" + cards[0].idx)
                                             : (gpu.integrated.empty() ? "" : gpu.integrated[0].card);
            if (!key.empty())
                drawGraph(S, row, ix, iw, std::min(left, 5), gpu.hist[key], C_GPU, 100);
        }
    }

    void pDisk(int y, int x, int h, int w) {
        frameBox(S, y, x, h, w, "儲存", C_FRAME, C_DISK,
                 "R " + human(disk.rdRate) + "/s  W " + human(disk.wrRate) + "/s");
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2, row = iy;
        for (auto& m : disk.mounts) {
            if (row - iy >= ih - 1) break;
            S.put(row, ix, dpad(trunc(m.mp, 16), 16), C_DISK);
            std::string txt = human((double)m.free_) + "/" + human((double)m.total);
            int bw = std::max(6, iw - 18 - dw(txt) - 6);
            bar(S, row, ix + 17, bw, m.pct);
            S.put(row, ix + 17 + bw + 1, fmt("%3.0f%%", m.pct), heat(m.pct), m.pct >= 90 ? A_BOLD : 0);
            S.put(row, ix + iw - dw(txt), txt, C_FAINT);
            row++;
        }
        if (ih - (row - iy) >= 1) {
            int sw = std::max(4, iw / 2 - 8);
            S.put(row, ix, "R " + sparkline(disk.rdHist, sw), C_OK);
            S.put(row, ix + iw / 2, "W " + sparkline(disk.wrHist, sw), C_WARN);
        }
    }

    void pNet(int y, int x, int h, int w) {
        frameBox(S, y, x, h, w, "網路", C_FRAME, C_NET,
                 "↓" + human(net.rx) + "/s ↑" + human(net.tx) + "/s");
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2, row = iy;
        std::vector<std::pair<std::string, Iface*>> act;
        for (auto& kv : net.ifaces)
            if (!kv.second.skip && kv.second.up) act.push_back({kv.first, &kv.second});
        std::sort(act.begin(), act.end(), [](auto& a, auto& b) {
            return (a.second->rxr + a.second->txr) > (b.second->rxr + b.second->txr);
        });
        for (auto& kv : act) {
            if (row - iy >= ih) break;
            S.put(row, ix, dpad(trunc(kv.first, 9), 9), C_NET, A_BOLD);
            S.put(row, ix + 10, "↓" + dpad(human(kv.second->rxr), 9, true) + "/s", C_OK);
            S.put(row, ix + 23, "↑" + dpad(human(kv.second->txr), 9, true) + "/s", C_WARN);
            int sw = iw - 38;
            if (sw > 4) S.put(row, ix + 37, sparkline(kv.second->rx, sw), C_NET);
            row++;
        }
        int left = ih - (row - iy);
        if (left >= 2) drawGraph(S, row, ix, iw, left, net.rxHist, C_NET);
    }

    void pProc(int y, int x, int h, int w) {
        frameBox(S, y, x, h, w, "行程", C_FRAME, C_PROC,
                 fmt("%d 個 / %d 執行緒", proc.total, proc.threads));
        int iw = w - 4, ix = x + 2, iy = y + 1, ih = h - 2;
        S.put(iy, ix, trunc(fmt("%7s %-9s %6s %9s  %s", "PID", "USER", "CPU%", "RSS", "COMMAND"), iw),
              C_DIM, A_UL);
        auto tp = topProcs(ih - 1);
        for (size_t i = 0; i < tp.size(); i++) {
            const ProcInfo* p = tp[i];
            int r = iy + 1 + (int)i;
            S.put(r, ix, fmt("%7d", p->pid), C_FAINT);
            S.put(r, ix + 8, dpad(p->user, 9), p->user == me ? C_ACCENT : C_DIM);
            S.put(r, ix + 18, fmt("%6.1f", p->cpu), heat(std::min(100.0, p->cpu)),
                  p->cpu > 50 ? A_BOLD : 0);
            S.put(r, ix + 25, dpad(human((double)p->rss), 9, true), C_MEM);
            S.put(r, ix + 36, trunc(p->name, std::max(1, iw - 36)), C_FG);
        }
    }

    // ── 總覽版面 ────────────────────────────────────────────────────────
    using PanelFn = void (App::*)(int, int, int, int);
    void column(int y, int x, int h, int w,
                const std::vector<std::pair<PanelFn, double>>& panels) {
        int used = 0;
        for (size_t i = 0; i < panels.size(); i++) {
            int ph = (i + 1 == panels.size()) ? (h - used)
                                              : std::max(4, (int)(h * panels[i].second));
            if (used + ph > h) ph = h - used;
            if (ph < 4) break;
            (this->*panels[i].first)(y + used, x, ph, w);
            used += ph;
        }
    }
    void drawOverview(int top, int h, int w) {
        if (w >= 132 && h >= 22) {
            int lw = w / 2, rw = w - lw;
            column(top, 0, h, lw, {{&App::pCpu, 0.42}, {&App::pGpu, 0.29}, {&App::pNet, 0.29}});
            column(top, lw, h, rw, {{&App::pMem, 0.30}, {&App::pDisk, 0.32}, {&App::pProc, 0.38}});
        } else if (w >= 96 && h >= 30) {
            column(top, 0, h, w, {{&App::pCpu, 0.26}, {&App::pMem, 0.15}, {&App::pGpu, 0.17},
                                  {&App::pDisk, 0.16}, {&App::pNet, 0.12}, {&App::pProc, 0.14}});
        } else {
            column(top, 0, h, w, {{&App::pCpu, 0.34}, {&App::pMem, 0.20},
                                  {&App::pGpu, 0.22}, {&App::pProc, 0.24}});
        }
    }

    // ── 深入頁面 ────────────────────────────────────────────────────────
    void dCpu(int top, int h, int w) {
        int gh = clampv(h / 3, 6, 11);
        frameBox(S, top, 0, gh, w, "總使用率 (%)", C_FRAME, C_CPU,
                 fmt("%.1f%% · %s · %s", cpu.pct, hzs(cpu.freqAvg(), cpu.freqAvg() > 0).c_str(),
                     cpu.temp >= 0 ? fmt("%.0f°C", cpu.temp).c_str() : "溫度 n/a"));
        drawGraph(S, top + 1, 2, w - 4, gh - 2, cpu.total, C_CPU, 100);
        int y = top + gh, rest = h - gh - 1, lw = (int)(w * 0.56);
        frameBox(S, y, 0, rest, lw, "每個邏輯核心", C_FRAME, C_CPU,
                 fmt("%d 核 / %d 執行緒", cpu.coreTemps.empty() ? cpu.n / 2
                                                               : (int)cpu.coreTemps.size(), cpu.n));
        int iw = lw - 4, percol = iw >= 62 ? 2 : 1, colw = iw / percol, rows = rest - 2;
        for (int i = 0; i < cpu.n; i++) {
            int r = rows ? i % rows : 0, cc = rows ? i / rows : 0;
            if (cc >= percol) break;
            int cy = y + 1 + r, cx = 2 + cc * colw;
            S.put(cy, cx, fmt("%2d", i), C_FAINT);
            int bw = std::max(6, colw - 26);
            bar(S, cy, cx + 3, bw, cpu.corePct[(size_t)i]);
            S.put(cy, cx + 3 + bw + 1, fmt("%5.1f%%", cpu.corePct[(size_t)i]),
                  heat(cpu.corePct[(size_t)i]));
            double f = (i < (int)cpu.freqs.size()) ? cpu.freqs[(size_t)i] : -1;
            S.put(cy, cx + colw - 10, dpad(hzs(f, f > 0), 9, true), C_DIM);
        }
        int rx = lw, rww = w - lw, sh = rest / 2;
        frameBox(S, y, rx, sh, rww, "核心統計", C_FRAME, C_CPU);
        std::vector<std::pair<std::string, std::string>> stats = {
            {"型號", trunc(cpu.model, rww - 18)},
            {"邏輯處理器", std::to_string(cpu.n)},
            {"平均頻率", hzs(cpu.freqAvg(), cpu.freqAvg() > 0)},
            {"封裝溫度", cpu.temp >= 0 ? fmt("%.1f °C", cpu.temp) : "n/a"},
            {"負載 1/5/15", fmt("%.2f  %.2f  %.2f", cpu.load[0], cpu.load[1], cpu.load[2])},
            {"每核負載", fmt("%.2f  (=load/核心數)", cpu.load[0] / std::max(1, cpu.n))},
            {"執行中 / 阻塞", fmt("%d / %d", cpu.running, cpu.blocked)},
            {"iowait", fmt("%.2f %%", cpu.iowait)},
            {"steal", fmt("%.2f %%", cpu.steal)},
            {"context switch", fmt("%.0f /s", cpu.ctxtRate)},
            {"中斷", fmt("%.0f /s", cpu.intrRate)},
            {"新行程", fmt("%.1f /s", cpu.forkRate)},
            {"開機至今", durs(cpu.uptime)},
        };
        for (size_t i = 0; i < stats.size() && (int)i < sh - 2; i++) {
            S.put(y + 1 + (int)i, rx + 2, dpad(stats[i].first, 14), C_DIM);
            S.put(y + 1 + (int)i, rx + 17, trunc(stats[i].second, rww - 19), C_FG, A_BOLD);
        }
        int y2 = y + sh, h2 = rest - sh;
        frameBox(S, y2, rx, h2, rww, "最耗 CPU 的行程", C_FRAME, C_CPU);
        auto tp = topProcs(h2 - 2);
        for (size_t i = 0; i < tp.size(); i++) {
            int r = y2 + 1 + (int)i;
            S.put(r, rx + 2, fmt("%6.1f%%", tp[i]->cpu), heat(std::min(100.0, tp[i]->cpu)), A_BOLD);
            S.put(r, rx + 10, dpad(tp[i]->user, 8), C_FAINT);
            S.put(r, rx + 19, trunc(tp[i]->name, rww - 21), C_FG);
        }
        hint(top + h - 1, 2, w - 4,
             fmt("load average 是「可執行 + 不可中斷等待」的行程數平均；除以 %d 顆核心才是真正的滿載程度。"
                 "iowait 高代表卡在磁碟而不是算不完。", cpu.n));
    }

    void dMem(int top, int h, int w) {
        int gh = clampv(h / 3, 6, 11), lw = w / 2;
        frameBox(S, top, 0, gh, lw, "RAM 使用率 (%)", C_FRAME, C_MEM,
                 human((double)mem.used()) + " / " + human((double)mem.total()));
        drawGraph(S, top + 1, 2, lw - 4, gh - 2, mem.hist, C_MEM, 100);
        frameBox(S, top, lw, gh, w - lw, "Swap 使用率 (%)", C_FRAME, C_MEM,
                 human((double)mem.swapUsed()) + " / " + human((double)mem.swapTotal()));
        drawGraph(S, top + 1, lw + 2, w - lw - 4, gh - 2, mem.swapHist, C_MEM, 100);
        int y = top + gh, rest = h - gh - 1;
        frameBox(S, y, 0, 4, w, "實體記憶體組成", C_FRAME, C_MEM);
        int iw = w - 4;
        long long tot = std::max(1LL, mem.total());
        long long buf = mem.g("Buffers");
        long long cache = std::max(0LL, mem.g("Cached") + mem.g("SReclaimable") - mem.g("Shmem"));
        struct Seg { const char* n; long long v; Col c; };
        Seg segs[4] = {{"used", mem.used(), C_CRIT}, {"buffers", buf, C_WARN},
                       {"cache", cache, C_COOL}, {"free", mem.g("MemFree"), C_OK}};
        int cx = 2;
        for (auto& sg : segs) {
            int n = (int)std::lround((double)sg.v / tot * iw);
            if (n <= 0) continue;
            n = std::min(n, 2 + iw - cx);
            if (n <= 0) break;
            std::string b;
            for (int i = 0; i < n; i++) b += "█";
            S.put(y + 1, cx, b, sg.c, A_BOLD);
            cx += n;
        }
        cx = 2;
        for (auto& sg : segs) {
            std::string lbl = std::string(sg.n) + " " + human((double)sg.v);
            if (cx + dw(lbl) + 2 > w - 2) break;
            S.put(y + 2, cx, "▪", sg.c, A_BOLD);
            S.put(y + 2, cx + 2, lbl, C_DIM);
            cx += dw(lbl) + 4;
        }
        y += 4; rest -= 4;
        int lw2 = (int)(w * 0.52);
        frameBox(S, y, 0, rest, lw2, "詳細 (/proc/meminfo)", C_FRAME, C_MEM);
        static const char* KEYS[] = {"MemTotal","MemFree","MemAvailable","Buffers","Cached",
            "SwapCached","Active","Inactive","Dirty","Writeback","AnonPages","Mapped","Shmem",
            "Slab","SReclaimable","KernelStack","PageTables","CommitLimit","Committed_AS",
            "VmallocUsed","HugePages_Total", nullptr};
        int col2 = std::max(1, rest - 2);
        for (int i = 0; KEYS[i]; i++) {
            int r = i % col2, cc = i / col2, cxx = 2 + cc * (lw2 / 2);
            if (cxx + 24 > lw2) break;
            auto it = mem.info.find(KEYS[i]);
            S.put(y + 1 + r, cxx, dpad(KEYS[i], 16), C_DIM);
            S.put(y + 1 + r, cxx + 17,
                  dpad(it == mem.info.end() ? "-" : human((double)it->second), 10, true), C_FG);
        }
        frameBox(S, y, lw2, rest, w - lw2, "最耗記憶體的行程", C_FRAME, C_MEM);
        int savedSort = sortKey; sortKey = 1;
        auto tp = topProcs(rest - 2);
        sortKey = savedSort;
        for (size_t i = 0; i < tp.size(); i++) {
            int r = y + 1 + (int)i;
            double pc = tp[i]->rss * 100.0 / (double)std::max(1LL, mem.total());
            S.put(r, lw2 + 2, dpad(human((double)tp[i]->rss), 9, true), C_MEM, A_BOLD);
            S.put(r, lw2 + 12, fmt("%4.1f%%", pc), heat(pc));
            S.put(r, lw2 + 18, dpad(tp[i]->user, 8), C_FAINT);
            S.put(r, lw2 + 27, trunc(tp[i]->name, w - lw2 - 29), C_FG);
        }
        hint(top + h - 1, 2, w - 4,
             "cache/buffers 是可回收的，Linux 拿空閒記憶體做磁碟快取是好事；"
             "真正該看的是 MemAvailable，而不是 MemFree。");
    }

    void dGpu(int top, int h, int w) {
        std::vector<NvCard> cards; std::vector<NvProc> gprocs; std::string err;
        gpu.snapshot(cards, gprocs, err);
        int y = top, avail = h - 1;
        if (!err.empty() || (cards.empty() && gpu.nvPresent)) {
            int dh = std::max(5, std::min(avail - 6, 4 + (int)gpu.diag.size()));
            frameBox(S, y, 0, dh, w, "NVIDIA 診斷", C_CRIT, C_CRIT, "nvidia-smi 目前不可用");
            S.put(y + 1, 2, trunc("✗ " + (err.empty() ? "nvidia-smi 無輸出" : err), w - 4),
                  C_CRIT, A_BOLD);
            for (size_t i = 0; i < gpu.diag.size(); i++) {
                int r = y + 2 + (int)i;
                if (r >= y + dh - 1) break;
                static const char* MK[4] = {"·", "✓", "!", "✗"};
                static const Col MC[4] = {C_DIM, C_OK, C_WARN, C_CRIT};
                int lv = gpu.diag[i].lvl;
                S.put(r, 2, MK[lv], MC[lv], A_BOLD);
                S.put(r, 4, trunc(gpu.diag[i].text, w - 6), MC[lv]);
            }
            y += dh; avail -= dh;
            int fh = std::min(avail - 4, 7);
            if (fh >= 4) {
                frameBox(S, y, 0, fh, w, "建議修法 (需要 root，會動到核心模組)", C_WARN, C_WARN);
                static const char* FIX[] = {
                    "1. 只留一個版本：sudo apt purge 'nvidia-dkms-545*'   # 移除舊的那份",
                    "2. 重建模組：    sudo dkms autoinstall -k $(uname -r)",
                    "3. 或整包重裝：  sudo apt install --reinstall nvidia-driver-550",
                    "4. 載入並驗證：  sudo modprobe nvidia && nvidia-smi",
                    "5. 若 Secure Boot 開著，模組要簽章，重開機後跑 MOK 註冊流程", nullptr};
                for (int i = 0; FIX[i] && i < fh - 2; i++)
                    S.put(y + 1 + i, 2, trunc(FIX[i], w - 4), C_DIM);
                y += fh; avail -= fh;
            }
        }
        for (auto& g : cards) {
            if (avail < 8) break;
            int ch = std::min(avail, 10);
            frameBox(S, y, 0, ch, w, "GPU " + g.idx + " · " + g.name, C_FRAME, C_GPU,
                     "driver " + g.driver + " · PCIe gen" + g.gen + " x" + g.width);
            int gw = (int)(w * 0.55);
            drawGraph(S, y + 1, 2, gw - 4, ch - 3, gpu.hist["nv" + g.idx], C_GPU, 100);
            S.put(y + ch - 2, 2, "使用率", C_DIM);
            bar(S, y + ch - 2, 9, gw - 20, g.util);
            S.put(y + ch - 2, gw - 10, fmt("%5.1f%%", g.util), heat(g.util), A_BOLD);
            std::vector<std::pair<std::string, std::string>> info = {
                {"VRAM", fmt("%s / %s (%.0f%%)", human((double)g.mused).c_str(),
                             human((double)g.mtotal).c_str(),
                             g.mtotal ? g.mused * 100.0 / g.mtotal : 0.0)},
                {"記憶體頻寬使用", fmt("%.0f %%", g.mutil)},
                {"溫度", g.temp >= 0 ? fmt("%.0f °C", g.temp) : "n/a"},
                {"功耗", (g.power >= 0 && g.plimit >= 0) ? fmt("%.1f / %.0f W", g.power, g.plimit) : "n/a"},
                {"SM 時脈", hzs(g.sclk, g.sclk >= 0)},
                {"記憶體時脈", hzs(g.mclk, g.mclk >= 0)},
                {"風扇", g.fan >= 0 ? fmt("%.0f %%", g.fan) : "n/a"},
                {"效能狀態", g.pstate},
                {"運算模式", g.cmode},
            };
            for (size_t i = 0; i < info.size() && (int)i < ch - 2; i++) {
                S.put(y + 1 + (int)i, gw + 2, dpad(info[i].first, 16), C_DIM);
                S.put(y + 1 + (int)i, gw + 19, trunc(info[i].second, w - gw - 21), C_FG, A_BOLD);
            }
            y += ch; avail -= ch;
        }
        for (auto& c : gpu.integrated) {
            if (avail < 6) break;
            if (c.vendor == "NVIDIA" && !cards.empty()) continue;
            int ch = std::min(avail, 8);
            frameBox(S, y, 0, ch, w, c.vendor + " · " + c.name, C_FRAME, C_GPU,
                     c.card + " · " + c.kind);
            int gw = (int)(w * 0.55);
            drawGraph(S, y + 1, 2, gw - 4, ch - 3, gpu.hist[c.card], C_GPU, 100);
            S.put(y + ch - 2, 2, "使用率", C_DIM);
            bar(S, y + ch - 2, 9, gw - 20, c.util < 0 ? 0 : c.util);
            S.put(y + ch - 2, gw - 10, c.util < 0 ? "  n/a" : fmt("%5.1f%%", c.util),
                  c.util < 0 ? C_FAINT : heat(c.util), c.util < 0 ? 0 : A_BOLD);
            std::vector<std::pair<std::string, std::string>> info;
            if (c.vramTotal > 0)
                info.push_back({"VRAM", human((double)c.vramUsed) + " / " + human((double)c.vramTotal)});
            info.push_back({"目前時脈", hzs((double)c.freq, c.freq >= 0)});
            info.push_back({"時脈範圍", hzs((double)c.fmin, c.fmin >= 0) + " ~ " +
                                        hzs((double)c.fmax, c.fmax >= 0)});
            info.push_back({"使用率來源", c.est ? "RC6 休眠殘留反推 (估計值)" : "gpu_busy_percent"});
            for (size_t i = 0; i < info.size() && (int)i < ch - 2; i++) {
                S.put(y + 1 + (int)i, gw + 2, dpad(info[i].first, 16), C_DIM);
                S.put(y + 1 + (int)i, gw + 19, trunc(info[i].second, w - gw - 21), C_FG, A_BOLD);
            }
            y += ch; avail -= ch;
        }
        if (!gprocs.empty() && avail >= 4) {
            int ph = std::min(avail, 2 + (int)gprocs.size());
            frameBox(S, y, 0, ph, w, "GPU 運算行程", C_FRAME, C_GPU);
            for (size_t i = 0; i < gprocs.size() && (int)i < ph - 2; i++) {
                S.put(y + 1 + (int)i, 2, dpad(gprocs[i].pid, 7, true), C_FAINT);
                S.put(y + 1 + (int)i, 11, dpad(human(gprocs[i].mem), 10, true), C_MEM);
                S.put(y + 1 + (int)i, 23, trunc(gprocs[i].name, w - 25), C_FG);
            }
            y += ph; avail -= ph;
        }
        if (cards.empty() && gpu.integrated.empty())
            S.put(y + 1, 2, "找不到任何 GPU 裝置。", C_DIM);
        hint(top + h - 1, 2, w - 4,
             "nvidia-smi 的「使用率」是取樣期間至少有一個 kernel 在跑的時間比例，"
             "不等於 CUDA core 的佔用率 — 跑滿 100% 不代表算力用好用滿。");
    }

    void dDisk(int top, int h, int w) {
        int y = top, avail = h - 1;
        int mh = std::min(std::max(5, 3 + (int)disk.mounts.size()), std::max(5, avail / 2));
        frameBox(S, y, 0, mh, w, "檔案系統掛載點", C_FRAME, C_DISK,
                 fmt("%d 個", (int)disk.mounts.size()));
        struct HCol { int x; const char* t; int rpad; };
        static const HCol HC[] = {{2,"掛載點",0},{25,"裝置",0},{40,"類型",0},{49,"總容量",10},
                                  {60,"已用",10},{71,"可用",10},{83,"使用率",0},{102,"%",4},
                                  {108,"inode",8}};
        for (auto& hc : HC) {
            if (hc.x + 4 > w - 2) break;
            S.put(y + 1, hc.x, hc.rpad ? dpad(hc.t, hc.rpad, true) : std::string(hc.t),
                  C_DIM, A_UL);
        }
        for (size_t i = 0; i < disk.mounts.size(); i++) {
            int r = y + 2 + (int)i;
            if (r >= y + mh - 1) break;
            const Mount& m = disk.mounts[i];
            S.put(r, 2, dpad(m.mp, 22), C_DISK, A_BOLD);
            std::string dv = m.dev;
            if (startsWith(dv, "/dev/")) dv = dv.substr(5);
            S.put(r, 25, dpad(dv, 14), C_FAINT);
            S.put(r, 40, dpad(m.fs, 8), C_FAINT);
            S.put(r, 49, dpad(human((double)m.total), 10, true), C_FG);
            S.put(r, 60, dpad(human((double)m.used), 10, true), C_DIM);
            S.put(r, 71, dpad(human((double)m.free_), 10, true), C_OK);
            bar(S, r, 83, 18, m.pct);
            S.put(r, 102, fmt("%3.0f%%", m.pct), heat(m.pct), m.pct >= 90 ? A_BOLD : 0);
            S.put(r, 108, fmt("%7.1f%%", m.ipct), heat(m.ipct));
        }
        y += mh; avail -= mh;
        std::vector<std::pair<std::string, DiskDev*>> devs;
        for (auto& kv : disk.devs) devs.push_back({kv.first, &kv.second});
        std::sort(devs.begin(), devs.end(), [](auto& a, auto& b) {
            return (a.second->rrate + a.second->wrate) > (b.second->rrate + b.second->wrate);
        });
        for (auto& kv : devs) {
            if (avail < 6) break;
            int ch = std::min(avail, 7);
            std::string model, kind; long long size = 0;
            disk.modelOf(kv.first, model, kind, size);
            auto tit = disk.temps.find(kv.first);
            std::string sub = kind + " · " + human((double)size);
            if (tit != disk.temps.end()) sub += fmt(" · %.0f°C", tit->second);
            frameBox(S, y, 0, ch, w, kv.first + " · " + model, C_FRAME, C_DISK, sub);
            int gw = (int)(w * 0.42);
            drawGraph(S, y + 1, 2, gw - 4, ch - 2, kv.second->rd_, C_OK);
            drawGraph(S, y + 1, gw, gw - 4, ch - 2, kv.second->wr, C_WARN);
            S.put(y + 1, 2, "讀 R", C_OK, A_BOLD);
            S.put(y + 1, gw, "寫 W", C_WARN, A_BOLD);
            int ix = gw * 2;
            std::pair<std::string, std::pair<std::string, Col>> rows[4] = {
                {"讀取", {human(kv.second->rrate) + "/s", C_OK}},
                {"寫入", {human(kv.second->wrate) + "/s", C_WARN}},
                {"IOPS", {fmt("%.0f", kv.second->iops), C_FG}},
                {"忙碌率", {fmt("%.1f %%", kv.second->util), heat(kv.second->util)}},
            };
            for (int i = 0; i < 4 && i < ch - 2; i++) {
                S.put(y + 1 + i, ix + 2, dpad(rows[i].first, 8), C_DIM);
                S.put(y + 1 + i, ix + 11, dpad(rows[i].second.first, 12, true),
                      rows[i].second.second, A_BOLD);
            }
            y += ch; avail -= ch;
        }
        hint(top + h - 1, 2, w - 4,
             "「忙碌率」是裝置有 I/O 在飛的時間比例，逼近 100% 就是磁碟塞車；"
             "SSD 因為能平行處理，忙碌率高不一定代表已達極限。");
    }

    void dNet(int top, int h, int w) {
        int y = top, avail = h - 1;
        int gh = clampv(h / 4, 6, 10), lw = w / 2;
        frameBox(S, y, 0, gh, lw, "下載 (bytes/s)", C_FRAME, C_NET, human(net.rx) + "/s");
        drawGraph(S, y + 1, 2, lw - 4, gh - 2, net.rxHist, C_OK);
        frameBox(S, y, lw, gh, w - lw, "上傳 (bytes/s)", C_FRAME, C_NET, human(net.tx) + "/s");
        drawGraph(S, y + 1, lw + 2, w - lw - 4, gh - 2, net.txHist, C_WARN);
        y += gh; avail -= gh;
        std::vector<std::pair<std::string, Iface*>> ord;
        for (auto& kv : net.ifaces) ord.push_back({kv.first, &kv.second});
        std::sort(ord.begin(), ord.end(), [](auto& a, auto& b) {
            if (a.second->skip != b.second->skip) return !a.second->skip;
            return a.first < b.first;
        });
        for (auto& kv : ord) {
            if (avail < 6) break;
            int ch = std::min(avail, 6);
            Iface* d = kv.second;
            std::string state = d->up ? "UP" : "DOWN";
            std::string sub = state;
            if (d->speed > 0) sub += fmt(" · %lld Mb/s", d->speed);
            frameBox(S, y, 0, ch, w, kv.first, C_FRAME, d->up ? C_NET : C_FAINT, sub);
            std::pair<std::string, std::pair<std::string, Col>> rows[4] = {
                {"狀態", {state, d->up ? C_OK : C_FAINT}},
                {"下載", {human(d->rxr) + "/s   累計 " + human((double)d->rxb), C_OK}},
                {"上傳", {human(d->txr) + "/s   累計 " + human((double)d->txb), C_WARN}},
                {"錯誤/丟包", {fmt("%lld / %lld", d->err, d->drop),
                               (d->err + d->drop) ? C_CRIT : C_DIM}},
            };
            for (int i = 0; i < 4 && i < ch - 2; i++) {
                S.put(y + 1 + i, 2, dpad(rows[i].first, 10), C_DIM);
                S.put(y + 1 + i, 13, dpad(rows[i].second.first, 34), rows[i].second.second);
            }
            S.put(y + 1, 50, "MAC", C_DIM);
            S.put(y + 1, 56, d->mac.empty() ? "-" : d->mac, C_FAINT);
            for (size_t i = 0; i < d->addrs.size() && (int)i < ch - 3; i++) {
                S.put(y + 2 + (int)i, 50, "IP", C_DIM);
                S.put(y + 2 + (int)i, 56, trunc(d->addrs[i], 40), C_ACCENT);
            }
            int sw = w - 100;
            if (sw > 10) {
                S.put(y + 1, 98, sparkline(d->rx, sw), C_OK);
                S.put(y + 2, 98, sparkline(d->tx, sw), C_WARN);
            }
            y += ch; avail -= ch;
        }
        if (avail >= 4) {
            int sh = std::min(avail, 5);
            frameBox(S, y, 0, sh, w, "Socket 統計 (/proc/net/sockstat)", C_FRAME, C_NET);
            int r = y + 1;
            std::string ss = rd("/proc/net/sockstat");
            eachLine(ss, [&](std::string_view l) {
                if (r >= y + sh - 1 || l.empty()) return;
                S.put(r++, 2, trunc(l, w - 4), C_DIM);
            });
        }
        hint(top + h - 1, 2, w - 4,
             "這裡的數字是 bytes/s。要換成一般講的 Mbps 要 ×8 再除以 1e6 —"
             " 千兆網路的理論上限約 125 MB/s。");
    }

    void dProc(int top, int h, int w) {
        int rows = h - 4;
        auto items = topProcs(0, true);
        long long totMem = std::max(1LL, mem.total());
        int maxs = std::max(0, (int)items.size() - rows);
        scroll = clampv(scroll, 0, maxs);
        static const char* SN[4] = {"cpu", "rss", "pid", "thr"};
        std::string title = fmt("行程 (排序: %s%s)", SN[sortKey],
                                filter.empty() ? "" : ("  篩選: " + filter).c_str());
        frameBox(S, top, 0, h - 1, w, title, C_FRAME, C_PROC,
                 fmt("%d/%d 顯示 · %d 執行緒", (int)items.size(), proc.total, proc.threads));
        struct C2 { const char* n; int w; int key; };
        static const C2 COLS[] = {{"PID",7,2},{"PPID",7,-1},{"USER",10,-1},{"S",2,-1},
                                  {"CPU%",6,0},{"MEM%",6,-1},{"RSS",10,1},{"VIRT",10,-1},
                                  {"THR",5,3},{"TIME",9,-1}};
        int x = 2;
        for (auto& c : COLS) {
            bool act = (c.key == sortKey);
            S.put(top + 1, x, dpad(c.n, c.w, true), act ? C_ACCENT : C_DIM,
                  A_UL | (act ? A_BOLD : 0));
            x += c.w + 1;
        }
        S.put(top + 1, x, "COMMAND", C_DIM, A_UL);
        int cmdx = x;
        for (int i = 0; i < rows; i++) {
            int idx = scroll + i;
            if (idx >= (int)items.size()) break;
            const ProcInfo* q = items[(size_t)idx];
            int r = top + 2 + i;
            bool mine = (q->user == me);
            double mp = q->rss * 100.0 / (double)totMem;
            std::pair<std::string, Col> vals[10] = {
                {fmt("%7d", q->pid), C_FAINT},
                {fmt("%7d", q->ppid), C_FAINT},
                {dpad(q->user, 10, true), mine ? C_ACCENT : C_DIM},
                {fmt("%2c", q->state), (q->state == 'D' || q->state == 'Z') ? C_CRIT : C_FAINT},
                {fmt("%6.1f", q->cpu), heat(std::min(100.0, q->cpu))},
                {fmt("%6.1f", mp), heat(mp)},
                {dpad(human((double)q->rss), 10, true), C_MEM},
                {dpad(human((double)q->vsz), 10, true), C_FAINT},
                {fmt("%5d", q->thr), C_FAINT},
                {dpad(durs(q->cputime), 9, true), C_FAINT},
            };
            x = 2;
            for (int c = 0; c < 10; c++) {
                S.put(r, x, vals[c].first, vals[c].second,
                      (vals[c].second == C_CRIT || vals[c].second == C_WARN) ? A_BOLD : 0);
                x += COLS[c].w + 1;
            }
            S.put(r, cmdx, trunc(q->cmd, std::max(1, w - cmdx - 2)), mine ? C_FG : C_DIM);
        }
        if (maxs)
            S.put(top + h - 2, w - 24, fmt("%d-%d / %d  ↑↓ PgUp/PgDn", scroll + 1,
                  std::min((int)items.size(), scroll + rows), (int)items.size()), C_FAINT);
        hint(top + h - 1, 2, w - 4,
             "狀態 S=睡眠 R=執行 D=不可中斷IO(通常是磁碟或NFS卡住) Z=殭屍。"
             "  按 s 換排序、/ 篩選。CPU% 可以超過 100%（多執行緒）。");
    }

    void drawHelp(int top, int h, int w) {
        int bw = std::min(w - 4, 84), bh = std::min(h - 1, 27);
        int bx = (w - bw) / 2, by = top + std::max(0, (h - bh) / 2);
        for (int i = 0; i < bh; i++) S.put(by + i, bx, std::string((size_t)bw, ' '));
        frameBox(S, by, bx, bh, bw, std::string("sysview ") + VERSION + " · 操作說明",
                 C_ACCENT, C_ACCENT);
        struct L { char k; const char* t; };
        static const L LS[] = {
            {'t', "頁面切換"},
            {'k', "~ 或 0     回到總覽（一頁看完 CPU/記憶體/GPU/磁碟/網路/行程）"},
            {'k', "1 CPU   2 記憶體   3 GPU   4 儲存   5 網路   6 行程"},
            {'k', "Tab / Shift-Tab  依序切換    ← →  同上"},
            {' ', ""},
            {'t', "通用"},
            {'k', "空白鍵        暫停 / 繼續取樣"},
            {'k', "+ / -         加快 / 放慢更新頻率 (0.2s ~ 10s)"},
            {'k', "r             立即重新取樣"},
            {'k', "? 或 h        開關本說明"},
            {'k', "q 或 Esc      離開（在深入頁面時先回到總覽）"},
            {' ', ""},
            {'t', "行程頁面 (6)"},
            {'k', "s             切換排序欄位 (cpu → rss → pid → thr)"},
            {'k', "/             輸入關鍵字篩選，Esc 清除"},
            {'k', "↑ ↓ PgUp PgDn Home End   捲動清單"},
            {' ', ""},
            {'t', "命令列參數"},
            {'k', "sysview -i 0.5        每 0.5 秒更新"},
            {'k', "sysview --snapshot    印出純文字快照後離開（可導到檔案 / 給 watch 用）"},
            {'k', "sysview -v gpu        直接開在某一頁"},
            {' ', ""},
            {'t', "資料全部來自 /proc 與 /sys，不需要 root；nvidia-smi 有裝才有 NVIDIA 數據。"},
            {0, nullptr},
        };
        for (int i = 0; LS[i].k; i++) {
            int r = by + 1 + i;
            if (r >= by + bh - 1) break;
            if (LS[i].k == 't') S.put(r, bx + 3, LS[i].t, C_ACCENT, A_BOLD);
            else if (LS[i].k == 'k') {
                std::string t = LS[i].t;
                size_t sp = t.find("  ");
                std::string a = (sp == std::string::npos) ? t : t.substr(0, sp);
                std::string b = (sp == std::string::npos) ? "" : t.substr(sp);
                S.put(r, bx + 5, a, C_WHITE, A_BOLD);
                if (!b.empty()) S.put(r, bx + 5 + dw(a), b, C_DIM);
            }
        }
    }

    void draw() {
        gpu.appsHot.store(view == V_GPU);
        S.clear();
        int h = S.H, w = S.W;
        if (h < 12 || w < 50) {
            S.put(0, 0, fmt("終端太小了 (需要至少 50x12，目前 %dx%d)", w, h), C_WARN, A_BOLD);
            return;
        }
        header(w);
        int top = 2, bh = h - 3;
        if (showHelp)            drawHelp(top, bh, w);
        else switch (view) {
            case V_OVER: drawOverview(top, bh, w); break;
            case V_CPU:  dCpu(top, bh, w);  break;
            case V_MEM:  dMem(top, bh, w);  break;
            case V_GPU:  dGpu(top, bh, w);  break;
            case V_DISK: dDisk(top, bh, w); break;
            case V_NET:  dNet(top, bh, w);  break;
            default:     dProc(top, bh, w); break;
        }
        footer(h, w);
    }
};

// ─────────────────────────────────────────────────────────────────────────────
// 終端機控制（原始模式 / 備用畫面 / 視窗大小）
// ─────────────────────────────────────────────────────────────────────────────

static struct termios g_orig;
static bool g_rawOn = false;
static volatile sig_atomic_t g_resized = 1;
static volatile sig_atomic_t g_quit = 0;

static void restoreTerm() {
    if (!g_rawOn) return;
    g_rawOn = false;
    const char* s = "\x1b[0m\x1b[?25h\x1b[?1049l";
    ssize_t u = write(STDOUT_FILENO, s, strlen(s));
    (void)u;
    tcsetattr(STDIN_FILENO, TCSAFLUSH, &g_orig);
}
static void onSig(int sig) {
    if (sig == SIGWINCH) { g_resized = 1; return; }
    g_quit = 1;
}
static bool rawMode() {
    if (!isatty(STDIN_FILENO)) return false;
    if (tcgetattr(STDIN_FILENO, &g_orig) != 0) return false;
    struct termios t = g_orig;
    t.c_lflag &= (tcflag_t)~(ECHO | ICANON);
    t.c_iflag &= (tcflag_t)~(IXON | ICRNL);
    t.c_cc[VMIN] = 0;
    t.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &t) != 0) return false;
    g_rawOn = true;
    atexit(restoreTerm);
    const char* s = "\x1b[?1049h\x1b[?25l\x1b[2J";
    ssize_t u = write(STDOUT_FILENO, s, strlen(s));
    (void)u;
    return true;
}
static void termSize(int& h, int& w) {
    struct winsize ws;
    if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == 0 && ws.ws_row && ws.ws_col) {
        h = ws.ws_row; w = ws.ws_col;
    } else { h = 24; w = 80; }
}

// 按鍵代碼（特殊鍵用負數，避開一般字元）
enum { K_NONE = 0, K_UP = -1, K_DOWN = -2, K_LEFT = -3, K_RIGHT = -4,
       K_PGUP = -5, K_PGDN = -6, K_HOME = -7, K_END = -8, K_BTAB = -9, K_ESC = -10 };

// 從一串已讀進來的位元組裡解出下一個按鍵
static int parseKey(const std::string& b, size_t& i) {
    if (i >= b.size()) return K_NONE;
    unsigned char c = (unsigned char)b[i];
    if (c != 0x1b) { i++; return (int)c; }
    if (i + 1 >= b.size()) { i++; return K_ESC; }
    if (b[i + 1] == '[' || b[i + 1] == 'O') {
        size_t j = i + 2;
        std::string num;
        while (j < b.size() && (isdigit((unsigned char)b[j]) || b[j] == ';')) num += b[j++];
        if (j >= b.size()) { i = b.size(); return K_ESC; }
        char f = b[j];
        i = j + 1;
        switch (f) {
            case 'A': return K_UP;
            case 'B': return K_DOWN;
            case 'C': return K_RIGHT;
            case 'D': return K_LEFT;
            case 'H': return K_HOME;
            case 'F': return K_END;
            case 'Z': return K_BTAB;
            case '~':
                if (num == "5") return K_PGUP;
                if (num == "6") return K_PGDN;
                if (num == "1" || num == "7") return K_HOME;
                if (num == "4" || num == "8") return K_END;
                return K_NONE;
            default: return K_NONE;
        }
    }
    i++;
    return K_ESC;
}

// 處理一個按鍵，回傳 false 代表該離開
static bool handleKey(App& a, int k) {
    if (k == K_NONE) return true;
    if (a.editing) {
        if (k == K_ESC)                    { a.editing = false; a.filter.clear(); }
        else if (k == '\r' || k == '\n')   { a.editing = false; }
        else if (k == 127 || k == 8)       { if (!a.filter.empty()) a.filter.pop_back(); }
        else if (k >= 32 && k < 127)       { a.filter += (char)k; }
        a.scroll = 0;
        return true;
    }
    if (a.showHelp && k != '?' && k != 'h' && k != K_ESC && k != 'q') a.showHelp = false;
    if (k == 'q' || k == 'Q' || k == K_ESC) {
        if (a.showHelp) a.showHelp = false;
        else if (a.view != V_OVER) { a.view = V_OVER; a.scroll = 0; }
        else return false;
        return true;
    }
    switch (k) {
        case '?': case 'h': case 'H': a.showHelp = !a.showHelp; break;
        case '`': case '0': case '~': a.view = V_OVER; a.scroll = 0; break;
        case '1': case '2': case '3': case '4': case '5': case '6':
            a.view = (View)(k - '0'); a.scroll = 0; break;
        case '\t': case K_RIGHT:
            a.view = (View)((a.view + 1) % V_COUNT); a.scroll = 0; break;
        case K_BTAB: case K_LEFT:
            a.view = (View)((a.view + V_COUNT - 1) % V_COUNT); a.scroll = 0; break;
        case ' ': a.paused = !a.paused; break;
        case '+': case '=':
            a.interval = std::max(0.2, a.interval - 0.2);
            a.note(fmt("更新間隔 %.1fs", a.interval)); break;
        case '-': case '_':
            a.interval = std::min(10.0, a.interval + 0.2);
            a.note(fmt("更新間隔 %.1fs", a.interval)); break;
        case 'r': case 'R': a.last = 0; break;
        case 's': if (a.view == V_PROC) a.sortKey = (a.sortKey + 1) % 4; break;
        case '/': if (a.view == V_PROC) { a.editing = true; a.filter.clear(); } break;
        case K_UP:   a.scroll = std::max(0, a.scroll - 1); break;
        case K_DOWN: a.scroll++; break;
        case K_PGUP: a.scroll = std::max(0, a.scroll - 20); break;
        case K_PGDN: a.scroll += 20; break;
        case K_HOME: a.scroll = 0; break;
        case K_END:  a.scroll = 1000000; break;
        default: break;
    }
    return true;
}

// ─────────────────────────────────────────────────────────────────────────────
// 純文字快照（無 TTY、導向檔案、或給 watch / cron 用）
// ─────────────────────────────────────────────────────────────────────────────

static std::string txtbar(double pct, int w = 28) {
    pct = clampv(std::isnan(pct) ? 0.0 : pct, 0.0, 100.0);
    int n = (int)(pct / 100.0 * w);
    return "[" + std::string((size_t)n, '#') + std::string((size_t)(w - n), '.') + "]";
}

static int snapshot(double interval) {
    Cpu cpu; Mem mem; Disk disk; Net net; Gpu gpu; Procs proc;
    gpu.start();
    cpu.update(0); mem.update(0); disk.update(0); net.update(0); gpu.update(0);
    proc.update(0, 1);
    struct timespec ts{(time_t)interval, (long)((interval - (long)interval) * 1e9)};
    nanosleep(&ts, nullptr);
    cpu.update(interval); mem.update(interval); disk.update(interval);
    net.update(interval); gpu.update(interval);
    proc.update(interval, std::max(1, cpu.n));
    net.readAddrs();

    std::string o;
    auto A = [&](const std::string& s) { o += s; o += "\n"; };
    char hn[256] = {0}; gethostname(hn, sizeof hn - 1);
    time_t t = time(nullptr); struct tm tmv; localtime_r(&t, &tmv);
    char tb[64]; strftime(tb, sizeof tb, "%F %T", &tmv);
    A(std::string(78, '='));
    A(fmt(" sysview %s   %s   %s   up %s", VERSION, hn, tb, durs(cpu.uptime).c_str()));
    A(std::string(78, '='));
    A("");
    A(fmt(" CPU  %s  %.1f%%   %s", txtbar(cpu.pct).c_str(), cpu.pct,
          trunc(cpu.model, 34).c_str()));
    A(fmt("      %d 執行緒 · %s · %s · load %.2f %.2f %.2f · iowait %.1f%%", cpu.n,
          hzs(cpu.freqAvg(), cpu.freqAvg() > 0).c_str(),
          cpu.temp >= 0 ? fmt("%.0f°C", cpu.temp).c_str() : "溫度 n/a",
          cpu.load[0], cpu.load[1], cpu.load[2], cpu.iowait));
    std::string line = "      ";
    for (int i = 0; i < cpu.n; i++) {
        line += fmt("%2d:%3.0f%%", i, cpu.corePct[(size_t)i]);
        if ((i + 1) % 8 == 0 || i + 1 == cpu.n) { A(line); line = "      "; }
        else line += "  ";
    }
    A("");
    A(fmt(" MEM  %s  %.1f%%   %s / %s", txtbar(mem.pct()).c_str(), mem.pct(),
          human((double)mem.used()).c_str(), human((double)mem.total()).c_str()));
    if (mem.swapTotal())
        A(fmt(" SWAP %s  %.1f%%   %s / %s", txtbar(mem.swapPct()).c_str(), mem.swapPct(),
              human((double)mem.swapUsed()).c_str(), human((double)mem.swapTotal()).c_str()));
    A(fmt("      cache %s · buffers %s · available %s",
          human((double)mem.g("Cached")).c_str(), human((double)mem.g("Buffers")).c_str(),
          human((double)mem.avail()).c_str()));
    A("");
    std::vector<NvCard> cards; std::vector<NvProc> gprocs; std::string err;
    gpu.snapshot(cards, gprocs, err);
    if (!cards.empty()) {
        for (auto& g : cards) {
            A(fmt(" GPU%s %s  %.1f%%   %s", g.idx.c_str(), txtbar(g.util).c_str(), g.util,
                  trunc(g.name, 34).c_str()));
            A(fmt("      VRAM %s / %s · %s · %s · %s",
                  human((double)g.mused).c_str(), human((double)g.mtotal).c_str(),
                  g.temp >= 0 ? fmt("%.0f°C", g.temp).c_str() : "n/a",
                  g.power >= 0 ? fmt("%.0fW", g.power).c_str() : "n/a",
                  hzs(g.sclk, g.sclk >= 0).c_str()));
        }
        for (auto& p : gprocs)
            A(fmt("      pid %-8s %10s  %s", p.pid.c_str(), human(p.mem).c_str(),
                  trunc(p.name, 40).c_str()));
    } else if (!err.empty()) {
        A(" GPU  [NVIDIA 不可用] " + err);
        static const char* MK[4] = {"-", "v", "!", "x"};
        for (auto& d : gpu.diag) A(fmt("      %s %s", MK[d.lvl], d.text.c_str()));
    }
    for (auto& c : gpu.integrated) {
        if (c.vendor == "NVIDIA" && !cards.empty()) continue;
        A(fmt(" iGPU %s  %s   %s · %s", txtbar(c.util < 0 ? 0 : c.util).c_str(),
              c.util < 0 ? " n/a " : fmt("%.1f%%", c.util).c_str(),
              c.name.c_str(), hzs((double)c.freq, c.freq >= 0).c_str()));
    }
    A("");
    A(fmt(" DISK  讀 %s/s  寫 %s/s", human(disk.rdRate).c_str(), human(disk.wrRate).c_str()));
    for (auto& m : disk.mounts)
        A(fmt("      %-20s %s %3.0f%%  %8s free / %8s", trunc(m.mp, 20).c_str(),
              txtbar(m.pct, 20).c_str(), m.pct, human((double)m.free_).c_str(),
              human((double)m.total).c_str()));
    A("");
    A(fmt(" NET   ↓ %s/s   ↑ %s/s", human(net.rx).c_str(), human(net.tx).c_str()));
    for (auto& kv : net.ifaces) {
        if (kv.second.skip || !kv.second.up) continue;
        std::string ips;
        for (size_t i = 0; i < kv.second.addrs.size() && i < 2; i++)
            ips += (i ? ", " : "") + kv.second.addrs[i];
        A(fmt("      %-10s ↓%10s/s ↑%10s/s   %s", kv.first.c_str(),
              human(kv.second.rxr).c_str(), human(kv.second.txr).c_str(), ips.c_str()));
    }
    A("");
    A(fmt(" PROC  %d 個行程 / %d 執行緒", proc.total, proc.threads));
    A(fmt("      %7s %-10s %6s %10s  %s", "PID", "USER", "CPU%", "RSS", "COMMAND"));
    std::vector<const ProcInfo*> v;
    for (auto& p : proc.list) v.push_back(&p);
    std::partial_sort(v.begin(), v.begin() + std::min<size_t>(10, v.size()), v.end(),
                      [](const ProcInfo* a, const ProcInfo* b) { return a->cpu > b->cpu; });
    for (size_t i = 0; i < v.size() && i < 10; i++)
        A(fmt("      %7d %-10s %6.1f %10s  %s", v[i]->pid, trunc(v[i]->user, 10).c_str(),
              v[i]->cpu, human((double)v[i]->rss).c_str(), trunc(v[i]->cmd, 40).c_str()));
    A("");
    gpu.stop();
    fwrite(o.data(), 1, o.size(), stdout);
    return 0;
}

// ─────────────────────────────────────────────────────────────────────────────

static const char* USAGE =
"sysview %s — 一體式系統儀表板 (CPU / 記憶體 / GPU / 儲存 / 網路 / 行程)\n"
"\n"
"用法:\n"
"  sysview                  開啟互動儀表板\n"
"  sysview -i 0.5           設定更新間隔（秒，預設 1.0）\n"
"  sysview -v cpu           直接開在指定頁面 (cpu|mem|gpu|disk|net|proc)\n"
"  sysview --snapshot       印出純文字快照後離開（可導向檔案、給 watch/cron 用）\n"
"  sysview --help           顯示本說明\n"
"\n"
"互動鍵:\n"
"  1-6 深入各項   ~ 回總覽   Tab 切頁   空白 暫停   +/- 更新率   ? 說明   q 離開\n"
"\n"
"資料來源皆為 /proc 與 /sys，不需要 root。NVIDIA 數據需要系統裝有 nvidia-smi。\n";

int main(int argc, char** argv) {
    double interval = 1.0;
    View view = V_OVER;
    bool snap = false;
    static const char* VN[V_COUNT] = {"overview","cpu","mem","gpu","disk","net","proc"};

    for (int i = 1; i < argc; i++) {
        std::string a = argv[i];
        if (a == "-h" || a == "--help") { printf(USAGE, VERSION); return 0; }
        if (a == "-V" || a == "--version") { printf("sysview %s\n", VERSION); return 0; }
        if ((a == "-i" || a == "--interval") && i + 1 < argc) {
            interval = clampv(atof(argv[++i]), 0.2, 60.0);
        } else if ((a == "-v" || a == "--view") && i + 1 < argc) {
            std::string vv = argv[++i];
            int found = -1;
            for (int k = 0; k < V_COUNT; k++) if (vv == VN[k]) found = k;
            if (found < 0) {
                fprintf(stderr, "錯誤: 未知頁面 '%s'\n", vv.c_str());
                return 2;
            }
            view = (View)found;
        } else if (a == "-s" || a == "--snapshot" || a == "--once") {
            snap = true;
        } else {
            fprintf(stderr, "未知參數: %s\n（-h 看說明）\n", a.c_str());
            return 2;
        }
    }

    const char* term = getenv("TERM");
    const char* ct = getenv("COLORTERM");
    g_has256 = ct != nullptr ||
               (term && (strstr(term, "256") || strstr(term, "truecolor") ||
                         strstr(term, "kitty") || strstr(term, "alacritty")));

    if (snap || !isatty(STDOUT_FILENO)) return snapshot(0.5);

    signal(SIGWINCH, onSig);
    signal(SIGINT, onSig);
    signal(SIGTERM, onSig);
    signal(SIGPIPE, SIG_IGN);
    if (!rawMode()) return snapshot(0.5);

    App app;
    app.interval = interval;
    app.view = view;

    std::string inbuf;
    char rb[512];
    app.last = nowMono();
    bool dirty = true;

    while (!g_quit) {
        if (g_resized) {
            g_resized = 0;
            int h, w;
            termSize(h, w);
            app.S.resize(h, w);
            app.S.forceRedraw();
            dirty = true;
        }
        if (dirty) {
            app.draw();
            app.S.flush(STDOUT_FILENO);
            dirty = false;
        }
        int timeoutMs = app.paused ? 200 : std::max(20, (int)(app.interval * 250));
        struct pollfd pfd{STDIN_FILENO, POLLIN, 0};
        int pr = poll(&pfd, 1, timeoutMs);
        if (pr > 0 && (pfd.revents & POLLIN)) {
            ssize_t n = read(STDIN_FILENO, rb, sizeof rb);
            if (n > 0) {
                inbuf.assign(rb, (size_t)n);
                size_t i = 0;
                while (i < inbuf.size()) {
                    int k = parseKey(inbuf, i);
                    if (!handleKey(app, k)) { g_quit = 1; break; }
                    dirty = true;
                }
            }
        }
        double now = nowMono();
        if (!app.paused && now - app.last >= app.interval) {
            app.sample(std::max(1e-3, now - app.last));
            app.last = now;
            dirty = true;
        }
        // 時鐘每秒要動，即使沒有其他變化
        static double lastClock = 0;
        if (now - lastClock >= 1.0) { lastClock = now; dirty = true; }
    }
    restoreTerm();
    return 0;
}
