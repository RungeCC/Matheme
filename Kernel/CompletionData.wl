(* ::Package:: *)

(* Pure data for FrontEnd argument-completion generation.

   scripts/generate-completions.wl enumerates the public symbols of the loaded
   package and reads option names from Options[f] or the generated
   $argumentTypes table.  The one fact it cannot infer is which endpoint takes
   a positional enumeration rather than option rules, so declare those action
   names here.  Keep this file free of side effects: the generator reads it
   with Get before loading the package. *)

$positionalCompletionActions = {"clearCache"};
