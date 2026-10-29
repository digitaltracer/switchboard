Fixtures from shipped app builds. `specs/build-N.json` is the API spec build N was generated
from; `requests/` holds request bodies those builds send. The app tests check the current hub
still serves them. Add a spec here when a build ships; never edit one.

`answer-message.json` pins the signed answer message and label hash; the hub's, the iPhone's and
Android's tests all check it.
