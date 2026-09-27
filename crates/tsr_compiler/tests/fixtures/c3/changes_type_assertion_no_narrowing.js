class C { p = 1; }
/** @param {C | undefined} cu */
function f(cu) {
  if (/** @type {any} */ (cu).undeclaredProperty) {
    cu.p;
  }
}
