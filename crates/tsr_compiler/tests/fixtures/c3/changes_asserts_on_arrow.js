class A { y = 0; }
class B extends A { z = 0; }
/**
 * @param {A} a
 * @returns { asserts a is B }
 */
const onArrow = (a) => {
  if (/** @type { B } */ (a).y !== 0) throw TypeError();
  return undefined;
};
/**
 * @type {(a: A) => asserts a is B}
 */
const onVariable = (a) => {
  if (/** @type { B } */ (a).y !== 0) throw TypeError();
  return undefined;
};
/** @param {A} a */
function use(a) {
  onVariable(a);
  a.z;
}
