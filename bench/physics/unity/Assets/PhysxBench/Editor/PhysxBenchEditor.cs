// Editor entry points of the R1 physics benchmark (docs/bench/physics-r1.md), run with
// -batchmode -nographics -projectPath bench/physics/unity -executeMethod <method>:
//
//   PhysxBenchEditor.BuildPlayers  Builds/PGS and Builds/TGS: Windows x64 players (Mono,
//                                  non-development), one per PhysX solver type, each with the
//                                  DynamicsManager.asset it was built from beside its .exe
//   PhysxBenchEditor.Run           the same benchmark inside the editor (edit mode), with the
//                                  project's current solver: -physxbench -scene ... (PhysxBench.cs)
//
// The solver type is ProjectSettings/DynamicsManager.asset's m_SolverType (0 PGS, 1 TGS), which no
// runtime API sets; BuildPlayers sets it for each build and restores it afterwards. Every other
// physics project setting stays Unity 6000.3.13f1's default for a new project (the committed
// ProjectSettings/DynamicsManager.asset); docs/bench/physics-r1.md, "Fidelity", lists them.
using System;
using System.IO;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEditor;
using UnityEditor.Build;
using UnityEditor.Build.Reporting;
using UnityEditor.SceneManagement;
using UnityEngine;

public static class PhysxBenchEditor
{
    const string DynamicsManager = "ProjectSettings/DynamicsManager.asset";
    const string ScenePath = "Assets/PhysxBench/Empty.unity";
    const string SolverFile = "Assets/StreamingAssets/physx_solver.txt";

    static SerializedObject Dynamics()
    {
        return new SerializedObject(AssetDatabase.LoadAllAssetsAtPath(DynamicsManager)[0]);
    }

    static string Solver()
    {
        var p = Dynamics().FindProperty("m_SolverType");
        if (p == null)
            throw new InvalidOperationException("DynamicsManager has no m_SolverType");
        return p.intValue == 1 ? "TGS" : "PGS";
    }

    static void SetSolver(string solver)
    {
        var so = Dynamics();
        so.FindProperty("m_SolverType").intValue = solver == "TGS" ? 1 : 0;
        so.ApplyModifiedPropertiesWithoutUndo();
        AssetDatabase.SaveAssets();
        if (Solver() != solver)
            throw new InvalidOperationException("could not set the solver type to " + solver);
    }

    public static void BuildPlayers()
    {
        int code = 0;
        string original = Solver();
        try
        {
            var scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
            EditorSceneManager.SaveScene(scene, ScenePath);
            // The players are Mono builds whatever the project's default (PhysxBench.cs records
            // the backend it was compiled for in each JSON line).
            PlayerSettings.SetScriptingBackend(NamedBuildTarget.Standalone, ScriptingImplementation.Mono2x);
            Directory.CreateDirectory(Path.GetDirectoryName(SolverFile));
            foreach (var solver in new[] { "PGS", "TGS" })
            {
                SetSolver(solver);
                File.WriteAllText(SolverFile, solver);
                AssetDatabase.Refresh();
                var report = BuildPipeline.BuildPlayer(new BuildPlayerOptions
                {
                    scenes = new[] { ScenePath },
                    locationPathName = "Builds/" + solver + "/PhysxBench.exe",
                    target = BuildTarget.StandaloneWindows64,
                    options = BuildOptions.None,
                });
                Debug.Log("physxbench build " + solver + ": " + report.summary.result + ", "
                          + report.summary.totalSize + " bytes");
                if (report.summary.result != BuildResult.Succeeded)
                {
                    code = 1;
                    break;
                }
                // The physics settings this player was built with, for the run's records.
                File.Copy(DynamicsManager, "Builds/" + solver + "/DynamicsManager.asset", true);
            }
        }
        catch (Exception e)
        {
            Debug.LogException(e);
            code = 1;
        }
        finally
        {
            SetSolver(original);
            AssetDatabase.DeleteAsset(Path.GetDirectoryName(SolverFile).Replace('\\', '/'));
            AssetDatabase.DeleteAsset(ScenePath);
        }
        EditorApplication.Exit(code);
    }

    public static void Run()
    {
        int code = 0;
        // PhysxBench.Run sets these project-wide physics settings; put them back so that an editor
        // run cannot leave them in ProjectSettings/DynamicsManager.asset for the next player build.
        var simulationMode = Physics.simulationMode;
        var gravity = Physics.gravity;
        int iterations = Physics.defaultSolverIterations;
        int velocityIterations = Physics.defaultSolverVelocityIterations;
        try
        {
            var o = PhysxBench.Parse(Environment.GetCommandLineArgs());
            if (o == null)
                throw new ArgumentException("PhysxBenchEditor.Run needs -physxbench and its options");
            o.mode = "editor";
            o.solver = Solver();
            PhysxBench.Run(o);
        }
        catch (Exception e)
        {
            Debug.LogException(e);
            code = 1;
        }
        finally
        {
            Physics.simulationMode = simulationMode;
            Physics.gravity = gravity;
            Physics.defaultSolverIterations = iterations;
            Physics.defaultSolverVelocityIterations = velocityIterations;
            JobsUtility.ResetJobWorkerCount();
        }
        EditorApplication.Exit(code);
    }
}
