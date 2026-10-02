package estransforms

// Phase 3 transform probe (overlay only): the package's unexported
// transformer factories by name, so a probe can run one downlevel transform
// alone. The chains of definitions.go are reached as "es".
import "github.com/microsoft/TypeScript/tsc/internal/transformers"

var Phase3Transformers = map[string]transformers.TransformerFactory{
	"using":             newUsingDeclarationTransformer,
	"esdecorator":       newESDecoratorTransformer,
	"classfields":       newClassFieldsTransformer,
	"logicalassignment": newLogicalAssignmentTransformer,
	"nullishcoalescing": newNullishCoalescingTransformer,
	"optionalchain":     newOptionalChainTransformer,
	"optionalcatch":     newOptionalCatchTransformer,
	"objectrestspread":  newObjectRestSpreadTransformer,
	"forawait":          newforawaitTransformer,
	"taggedtemplate":    newTaggedTemplateLiftRestrictionTransformer,
	"async":             newAsyncTransformer,
	"exponentiation":    newExponentiationTransformer,
}
