# sysview — 一體式系統儀表板
CXX      ?= g++
CXXFLAGS ?= -O2 -std=c++17 -Wall -Wextra -pipe
LDLIBS   ?= -lpthread
PREFIX   ?= /usr/local

all: sysview

sysview: sysview.cpp
	$(CXX) $(CXXFLAGS) -o $@ $< $(LDLIBS)
	-strip $@

install: sysview
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 0755 sysview $(DESTDIR)$(PREFIX)/bin/sysview

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/sysview

clean:
	rm -f sysview

.PHONY: all install uninstall clean
