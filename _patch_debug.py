with open(r'E:\my\proj\read_bin_cli\read_bin.py', 'r', encoding='utf-8') as f:
    content = f.read()
old = '        key = stdscr.getch()'
new = "        key = stdscr.getch()\n        with open('_keydbg.log','a') as _f:_f.write(f'{key}\\n')"
if content.count(old) == 1:
    content = content.replace(old, new)
    with open(r'E:\my\proj\read_bin_cli\read_bin.py', 'w', encoding='utf-8') as f:
        f.write(content)
    print('done')
else:
    print(f'found {content.count(old)} occurrences, aborting')
