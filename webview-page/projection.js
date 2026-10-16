/* One immutable selected phrase, identified by the native delivery token.
 * Missing sources never combine with newer transient editor state. */
const CrestTrackerProjection = (() => {
  "use strict";
  class PhraseReceiver {
    constructor() { this.source = null; this.phrase = null; this.missing = undefined; }
    receive(document) {
      const source = document.phraseSource;
      if (!source || typeof source.token !== "string" || source.revision !== document.revision) {
        return this.resync(source?.token ?? null);
      }
      const supplied = Object.prototype.hasOwnProperty.call(document, "phrase");
      if (supplied) {
        if ((document.phrase?.id ?? null) !== source.phrase) return this.resync(source.token);
        this.source = source;
        this.phrase = document.phrase;
        this.missing = undefined;
      }
      if (!this.source || this.source.token !== source.token ||
          this.source.revision !== source.revision || this.source.phrase !== source.phrase) {
        return this.resync(source.token);
      }
      return {model: {...document, phrase: this.phrase}, acknowledge: supplied ? source.token : null};
    }
    resync(token) {
      const request = this.missing !== token;
      this.missing = token;
      return {resync: request};
    }
  }
  return {PhraseReceiver};
})();
