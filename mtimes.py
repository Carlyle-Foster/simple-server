#!/bin/env python3

from os         import rename, remove
from os.path    import getmtime, isdir
from itertools  import islice
from pathlib    import Path
from subprocess import run

#polyfill for python versions before 3.12, which was released october 2023
def batched(iterable, n):
    iterator = iter(iterable)
    while batch := tuple(islice(iterator, n)):
        yield batch


def entry(path, last_modified):
     return f'{path}\n{last_modified:.3f} seconds\n\n'

watched_dir = Path('src')
my_name = watched_dir.joinpath('.modtimes')
clog_path = watched_dir.joinpath('.changelog')
dot_old = my_name.with_suffix('.old')
paths = set()
updated = []
deleted = []
try:
    rename(my_name, dot_old)
except FileNotFoundError as _e:
    # create an empty .old file to bootstrap
    open(dot_old, 'a').close()

# detect changes in the watched directory
with open(my_name, 'w') as mtimes:
    with open(dot_old, 'r') as old_mtimes:
        entries = batched(old_mtimes.readlines(), 3)
        for (path, mtime, _) in entries:
            path = Path(path[:-1])
            paths.add(path)
            try:
                last_modified = getmtime(path) # this file might have been deleted..
                mtimes.write(entry(path, last_modified))
                if isdir(path):
                    continue
                old_mtime = mtime.split()[0]
                new_mtime = f'{last_modified:.3f}'
                if new_mtime != old_mtime:
                    updated.append(path)
            except FileNotFoundError as _e:
                # ..if it was, report it as such
                deleted.append(path)
    remove(dot_old)
    for path in watched_dir.rglob('*'):
        if path not in paths:
            if path == my_name or path == clog_path:
                continue
            updated.append(path)
            last_modified = getmtime(path)
            mtimes.write(entry(path, last_modified))


def send_to_server(path):
    dest = path.parent if isdir(path) else path
    run(['cp', '-r', '--update', path, 'fake_server' / dest ])

def delete_from_server(path):
    run(['rm', '-dr', 'fake_server' / path])

# mirror changes over to the production server  
changelog = ''
dirs_updated = set()
for path in updated:
    if path.parent not in dirs_updated: 
        send_to_server(path)
    if isdir(path):
        dirs_updated.add(path)
    else:
        path = Path(*path.parts[1:])
        changelog += f'{path}\n\n'
dirs_deleted = set()
for path in deleted:
    if path.parent not in dirs_deleted: 
        delete_from_server(path)
    dirs_deleted.add(path)
    path = Path(*path.parts[1:])
    changelog += f'DELETE\t{path}\n\n'
if changelog != '':
    # overwrite the changelog to reflect the latest changes
    clog_path.write_text(changelog[:-2], newline='\n')
    send_to_server(clog_path)
    
    print(f'CHANGELOG:\n\n{changelog}', end='')
