import { createContext } from "react";

import type { LibrarySummary } from "./types";

export const LibraryContext = createContext<LibrarySummary | null>(null);
