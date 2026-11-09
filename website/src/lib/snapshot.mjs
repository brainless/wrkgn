import path from 'node:path';
import { loadSnapshot } from '../../lib/public-snapshot.mjs';

// Module-level load keeps one validated publication selection per Astro build.
export const snapshot = loadSnapshot(path.resolve(process.cwd(), 'data/public'));
