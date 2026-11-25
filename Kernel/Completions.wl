(* ::Package:: *)

(* Immediate-load FrontEnd completions.

   AutoCompletionData/specialArgFunctions.tr is generated from Kernel/Core.wl
   ($displayNames) and the generated argument types by scripts/generate-completions.wl
   and declared through the {"AutoCompletionData", Root -> "AutoCompletionData"}
   paclet extension.  PacletManager adds that root to the FrontEnd's
   PrivatePaths -> "AutoCompletionData", and the FrontEnd merges the file
   into its specialArgFunctions table (the same mechanism used by the
   EntityFramework, UnitTable, GraphStore, ResourceAutoCompletionData and
   WSM paclets).

   Re-registering those same rules here via FE`Evaluate makes completions
   available immediately after PacletDirectoryLoad / Get in an already
   running FrontEnd, and on the direct Get["Kernel/Matheme.wl"] dev path.
   No completion data is duplicated: this file only reads the .tr rules.

   FE`Evaluate forwards its argument to the FrontEnd process, so every rule
   must contain literal strings/lists; With injects them.  Everything is
   best-effort: headless kernels skip the block, a missing .tr file is
   ignored, and Quiet/Check keeps a missing or changed FrontEnd symbol from
   breaking Get["Runge`Matheme`"]. *)

If[TrueQ[$Notebooks] || MatchQ[Quiet[$FrontEnd], _FrontEndObject],

  With[
    {
      (* Kernel/Completions.wl -> paclet root/AutoCompletionData/... *)
      trFile = FileNameJoin[{
        DirectoryName[$InputFileName, 2], "AutoCompletionData",
        "specialArgFunctions.tr"}]
    },
    If[FileExistsQ[trFile],
      With[{rules = Quiet[Check[Get[trFile], {}]]},
        If[ListQ[rules],
          Scan[
            Function[rule,
              With[{r = rule},
                Quiet @ Check[
                  FE`Evaluate[FEPrivate`AddSpecialArgCompletion[r]],
                  $Failed
                ]
              ]
            ],
            rules
          ]
        ]
      ]
    ]
  ]
]
