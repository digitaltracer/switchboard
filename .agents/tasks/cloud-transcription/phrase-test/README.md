# Phrase test

Compares how the expected names reach `gpt-transcribe`, and whether a Claude correction afterwards earns a second key. The owner runs it; each variant is billed to the owner's keys.

1. Make a folder outside any git working tree. The script refuses one inside.
2. Record about 25 technical phrases there, with project, branch and agent names in them. Any format `afconvert` or `ffmpeg` reads will do.
3. Add `phrases.json` to that folder, shaped like `phrases.example.json`. `names` are the names that must come out exactly as written. `meaning` lists the words the command's meaning turns on, such as "don't" or "main". Mark at least five phrases `held_out`.
4. Make a names file, one name per line, most relevant first. Copy what the app's expected names hold: project ids and aliases, session labels and branches, agent names.
5. Check the inputs. This converts the recordings and sends nothing:

   ```sh
   python3 phrase_test.py --recordings ~/phrases --names ~/phrases/names.txt --dry-run
   ```

6. Run it. The keys come from the environment only, and `pip install anthropic` is needed for the Claude variant. `--skip-claude` leaves that variant out.

   ```sh
   python3 phrase_test.py --recordings ~/phrases --names ~/phrases/names.txt
   ```

It overwrites `results.md` here with the scores and transcripts. Read the transcripts too: they can show meaning changes that the manifest's word list misses. Then write the three decisions in the plan's As built section.
