#!/usr/bin/env python3
"""Compare rendered public content, without browser automation or private data."""
import argparse
import json
import re
from html.parser import HTMLParser
from pathlib import Path
from urllib.request import urlopen

class Node:
    def __init__(self, tag='', attrs=()):
        self.tag, self.attrs, self.children = tag, dict(attrs), []
    def text(self):
        if 'builder-convention' in self.attrs.get('class', '').split():
            return ''  # Additional explicit attribution convention in the Rust port.
        return ' '.join(c.text() if isinstance(c, Node) else c for c in self.children)
    def regions(self, cls):
        if cls in self.attrs.get('class', '').split():
            return [self]
        return [n for c in self.children if isinstance(c, Node) for n in c.regions(cls)]
    def links(self):
        return ([self.attrs['href']] if self.tag == 'a' and 'href' in self.attrs else []) + [u for c in self.children if isinstance(c, Node) for u in c.links()]

class Parser(HTMLParser):
    def __init__(self, html):
        super().__init__(convert_charrefs=True)
        self.root = Node()
        self.stack = [self.root]
        self.feed(html)
    def handle_starttag(self, tag, attrs):
        node = Node(tag, attrs)
        self.stack[-1].children.append(node)
        if tag not in {'area','base','br','col','embed','hr','img','input','link','meta','param','source','track','wbr'}:
            self.stack.append(node)
    def handle_endtag(self, tag):
        for i in range(len(self.stack) - 1, 0, -1):
            if self.stack[i].tag == tag:
                self.stack = self.stack[:i]
                return
    def handle_data(self, value):
        self.stack[-1].children.append(value)

def normalized(node):
    text = re.sub(r'\b0+(\d+)\s+(mentions?)\b', r'\1 \2', node.text())
    # Ignore typography and whitespace choices, retain actual words and dates.
    return re.sub(r'[\s“”"]+', '', text), node.links()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--snapshot', default='website/data/public')
    p.add_argument('--astro', default='website/dist')
    p.add_argument('--origin', default='http://127.0.0.1:3000')
    p.add_argument('--limit', type=int, default=0, help='0 compares all project pages')
    a = p.parse_args()
    root = Path(a.snapshot)
    pointer = json.loads((root / 'current.json').read_text())
    rows = json.loads((root / 'snapshots' / pointer['version'] / 'discovery.json').read_text())
    pages = ['/' if n == 1 else f'/projects/page/{n}/' for n in range(1, max(1, (len(rows)+29)//30)+1)]
    pages += [f'/projects/{r["slug"]}/' for r in (rows[:a.limit] if a.limit else rows)]
    failures = []
    for url in pages:
        static = (Path(a.astro) / url.lstrip('/') / 'index.html').read_text()
        with urlopen(a.origin.rstrip('/') + url) as response:
            live = response.read().decode()
        if f'data-snapshot-version="{pointer["version"]}"' not in live:
            raise RuntimeError('Server and Astro snapshot versions differ')
        cls = 'project-detail' if '/projects/' in url and '/page/' not in url else 'project-card'
        left = [normalized(n) for n in Parser(static).root.regions(cls)]
        right = [normalized(n) for n in Parser(live).root.regions(cls)]
        if left != right:
            failures.append(url)
            print('DIFF', url)
    print(f'Compared {len(pages)} pages; {len(failures)} differences')
    raise SystemExit(bool(failures))

if __name__ == '__main__':
    main()
