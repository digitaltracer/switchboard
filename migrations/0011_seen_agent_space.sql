-- The space (Herdr workspace, or Herdr app space) an agent ran in, so one that ended stays under it.
ALTER TABLE seen_agents ADD COLUMN space TEXT;
