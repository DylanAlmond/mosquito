import { describe, expect, it } from 'vitest';
import { groupSharedFiles } from './folderGroups';
import type { SharedFile } from '../types';

function file(id: number, name: string, provenance: SharedFile['provenance']): SharedFile {
  return { id, name, path: `C:/pics/${name}`, size: 7, provenance };
}

function member(id: number, relativePath: string, folderRoot: string): SharedFile {
  return { id, name: relativePath.split('/').pop()!, path: `${folderRoot}/${relativePath}`, size: 7, provenance: { folder_root: folderRoot, relative_path: relativePath } };
}

describe('groupSharedFiles', () => {
  it('puts loose files at the very front, ungrouped', () => {
    const root = 'C:/Photos';
    const files = [
      member(1, 'beach.jpg', root),
      file(2, 'notes.txt', null)
    ];

    const { loose, groups } = groupSharedFiles(files);

    expect(groups).toHaveLength(1);
    expect(loose.map((f) => f.id)).toEqual([2]);
  });

  it('groups members under their folder root, keeping first-appearance order of roots', () => {
    const photos = 'C:/Photos';
    const docs = 'C:/Docs';
    const files = [
      member(1, 'a.txt', docs),
      member(2, 'vacation/beach.jpg', photos),
      member(3, 'b.txt', docs)
    ];

    const { groups } = groupSharedFiles(files);

    expect(groups.map((g) => g.folderRoot)).toEqual([docs, photos]);
    expect(groups[0]!.files.map((f) => f.id)).toEqual([1, 3]);
    expect(groups[1]!.files.map((f) => f.id)).toEqual([2]);
  });

  it('names a group after the folder root itself, not the first member', () => {
    const root = 'C:/Users/me/Pictures/Holiday Snaps';
    const files = [member(1, 'day1/beach.jpg', root)];

    const { groups } = groupSharedFiles(files);

    expect(groups[0]!.name).toBe('Holiday Snaps');
  });

  it('keeps same-named folder roots as separate groups', () => {
    const files = [
      member(1, 'a.txt', 'C:/one/Photos'),
      member(2, 'b.txt', 'D:/two/Photos')
    ];

    const { groups } = groupSharedFiles(files);

    expect(groups).toHaveLength(2);
    expect(groups.map((g) => g.folderRoot)).toEqual(['C:/one/Photos', 'D:/two/Photos']);
    expect(groups.map((g) => g.name)).toEqual(['Photos', 'Photos']);
  });
});
