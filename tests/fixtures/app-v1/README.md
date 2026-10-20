Fixtures from shipped app builds. `specs/build-N.json` is the API spec build N was generated
from; `requests/` holds request bodies those builds send. The app tests check the current hub
still serves them. Add a spec here when a build ships; never edit one.
