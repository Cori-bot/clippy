import type { ClipMeta } from './types';

export type ClipSort = 'newest' | 'oldest' | 'duration' | 'size';

export type LibraryCriteria = {
  searchQuery: string;
  gameFilter: string | null;
  sortOrder: ClipSort;
  favoriteIds: Set<string> | null;
};

function searchable(value: string): string {
  return value.normalize('NFD').replace(/\p{M}/gu, '').toLocaleLowerCase();
}

/** All words can match anywhere across the title and game name. */
export function queryLibrary(clips: ClipMeta[], criteria: LibraryCriteria): ClipMeta[] {
  const words = searchable(criteria.searchQuery).trim().split(/\s+/).filter(Boolean);
  const result = clips.filter(clip => {
    if (criteria.favoriteIds && !criteria.favoriteIds.has(clip.id)) return false;
    if (criteria.gameFilter !== null && clip.game !== criteria.gameFilter) return false;
    const text = searchable(`${clip.title} ${clip.game ?? ''}`);
    return words.every(word => text.includes(word));
  });
  const created = (clip: ClipMeta) => Date.parse(clip.created) || 0;
  return result.sort((a, b) => {
    switch (criteria.sortOrder) {
      case 'oldest': return created(a) - created(b);
      case 'duration': return b.duration_ms - a.duration_ms || created(b) - created(a);
      case 'size': return b.bytes - a.bytes || created(b) - created(a);
      default: return created(b) - created(a);
    }
  });
}

/** Store inputs are replaced, never mutated, so identity is sufficient. */
export function createLibraryQuery() {
  let previous: (LibraryCriteria & { clips: ClipMeta[]; result: ClipMeta[] }) | undefined;
  return (clips: ClipMeta[], criteria: LibraryCriteria): ClipMeta[] => {
    if (previous && previous.clips === clips && previous.searchQuery === criteria.searchQuery &&
      previous.gameFilter === criteria.gameFilter && previous.sortOrder === criteria.sortOrder &&
      previous.favoriteIds === criteria.favoriteIds) return previous.result;
    const result = queryLibrary(clips, criteria);
    previous = { clips, ...criteria, result };
    return result;
  };
}
