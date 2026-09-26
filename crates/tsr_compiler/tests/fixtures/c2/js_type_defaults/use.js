/** @type {import('./classes').UnknownDefault} */
const unknownDefault = /** @type {any} */ ({});
/** @type {import('./classes').EmptyDefault} */
const emptyDefault = /** @type {any} */ ({});
/** @type {import('./classes').StructuralDefault} */
const structuralDefault = /** @type {any} */ ({});
/** @type {import('./classes').StringDefault} */
const stringDefault = /** @type {any} */ ({});
/** @type {import('./classes').DependentDefault} */
const dependentDefault = /** @type {any} */ ({});
/** @type {import('./classes').UnknownDefault<unknown>} */
const explicitUnknown = /** @type {any} */ ({});
/** @type {import('./classes').EmptyDefault<{}>} */
const explicitEmpty = /** @type {any} */ ({});

/** @type {import('./classes').DependentDefault<unknown>} */
const dependentExplicitUnknown = /** @type {any} */ ({});
/** @type {import('./classes').DependentDefault<{}>} */
const dependentExplicitEmpty = /** @type {any} */ ({});

/** @type {never} */ const observeUnknown = unknownDefault.value;
/** @type {never} */ const observeEmpty = emptyDefault.value;
/** @type {never} */ const observeStructural = structuralDefault.value;
/** @type {never} */ const observeString = stringDefault.value;
/** @type {never} */ const observeDependent = dependentDefault.value;
/** @type {never} */ const observeExplicitUnknown = explicitUnknown.value;
/** @type {never} */ const observeExplicitEmpty = explicitEmpty.value;
/** @type {never} */ const observeDependentExplicitUnknown = dependentExplicitUnknown.value;
/** @type {never} */ const observeDependentExplicitEmpty = dependentExplicitEmpty.value;
export {};
