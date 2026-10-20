-- Short approval numbers for chat (`approve 7`): assigned at creation, never reused.
ALTER TABLE approvals ADD COLUMN number INTEGER;
CREATE UNIQUE INDEX approvals_number ON approvals (number);
