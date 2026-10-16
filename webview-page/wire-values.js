/* Host-only exact u64 transport. BigInt never reaches Tauri's JSON serializer. */
const CrestTrackerWireValues = (() => {
  "use strict";
  const tag = "$crestU64", maximum = 18446744073709551615n;

  function unsignedInteger(value) {
    if (typeof value === "number" && !Number.isSafeInteger(value)) throw Error("Integer has lost precision");
    const text = String(value);
    if (!/^(?:0|[1-9]\d*)$/.test(text)) throw Error("Enter an unsigned decimal integer");
    const integer = BigInt(text);
    if (integer > maximum) throw Error("Integer exceeds 18446744073709551615");
    return integer <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(integer) : integer;
  }

  function stringify(value) {
    return JSON.stringify(value, (_, child) => typeof child === "bigint"
      ? { [tag]: String(unsignedInteger(child)) } : child);
  }

  function encode(value) {
    return JSON.parse(stringify(value));
  }

  // Projections are fresh objects. Revive in place without a second document copy.
  function decode(value) {
    if (!value || typeof value !== "object") return value;
    if (Object.hasOwn(value, tag) && typeof value[tag] === "string") {
      if (Object.keys(value).length !== 1 || typeof value[tag] !== "string") throw Error("Invalid exact integer");
      return unsignedInteger(value[tag]);
    }
    for (const key of Object.keys(value)) value[key] = decode(value[key]);
    return value;
  }

  return Object.freeze({ unsignedInteger, stringify, encode, decode });
})();
