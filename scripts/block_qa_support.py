"""Small, side-effect-free helpers shared by privileged QA and regression tests."""
def find_apfs_partition(document, loop):
    if not loop.startswith('/dev/loop') or not loop[9:].isdigit():
        raise ValueError('Expected the just-created loop device')
    pending=list(document.get('blockdevices', []));matches={}
    while pending:
        row=pending.pop()
        pending.extend(row.get('children') or [])
        path=row.get('path', '')
        suffix=path[len(loop):] if path.startswith(loop) else ''
        if row.get('type')=='part' and row.get('fstype')=='apfs' and suffix.startswith('p') and suffix[1:].isdigit():
            matches[path]=row
    if len(matches)!=1:
        raise RuntimeError(f'Expected exactly one APFS partition under {loop}; found {list(matches)}. See lsblk.json in the QA evidence directory.')
    return next(iter(matches.values()))
