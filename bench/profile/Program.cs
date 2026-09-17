using System.Text.Json;
using Microsoft.Diagnostics.Symbols;
using Microsoft.Diagnostics.Tracing.Etlx;
using Microsoft.Diagnostics.Tracing.Parsers.Kernel;
using Microsoft.Diagnostics.Tracing.Stacks;

if (args.Length != 4 || (args[3] != "cpu" && args[3] != "io"))
    throw new ArgumentException("usage: TraceSummary <trace.etl> <trusted-pdb-directory> <summary.json> <cpu|io>");

using var log = TraceLog.OpenOrConvert(Path.GetFullPath(args[0]),
    new TraceLogOptions { ConversionLog = Console.Error });
var process = log.Processes.Single(p =>
    string.Equals(Path.GetFileNameWithoutExtension(p.Name), "rip", StringComparison.OrdinalIgnoreCase));
var cache = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(args[2]))!, "symbols");
Directory.CreateDirectory(cache);
using var reader = new SymbolReader(Console.Error,
    $"{Path.GetFullPath(args[1])};srv*{cache}*https://msdl.microsoft.com/download/symbols");
// Only analyze our CI-owned executable/PDB and Microsoft public symbols.
reader.SecurityCheck = _ => true;
var stacks = new TraceEventStackSource(process.EventsInProcess.Filter(e => e is SampledProfileTraceData))
{
    ShowUnknownAddresses = true
};
stacks.LookupWarmSymbols(1, reader, stacks);

var inclusive = new Dictionary<string, double>();
var exclusive = new Dictionary<string, double>();
var cpuTimeline = new SortedDictionary<int, double>();
double sampledCpuMs = 0;
int samples = 0, samplesWithoutStacks = 0;
stacks.ForEach(sample =>
{
    samples++;
    sampledCpuMs += sample.Metric;
    int bucket = (int)((sample.TimeRelativeMSec - process.StartTimeRelativeMsec) / 100);
    cpuTimeline[bucket] = cpuTimeline.GetValueOrDefault(bucket) + sample.Metric;
    var stack = sample.StackIndex;
    if (stack == StackSourceCallStackIndex.Invalid)
        samplesWithoutStacks++;
    var seen = new HashSet<string>();
    bool leaf = true;
    while (stack != StackSourceCallStackIndex.Invalid)
    {
        string frame = stacks.GetFrameName(stacks.GetFrameIndex(stack), false);
        if (seen.Add(frame))
            inclusive[frame] = inclusive.GetValueOrDefault(frame) + sample.Metric;
        if (leaf)
            exclusive[frame] = exclusive.GetValueOrDefault(frame) + sample.Metric;
        leaf = false;
        stack = stacks.GetCallerIndex(stack);
    }
});

var operations = new SortedDictionary<string, long>();
var pending = new Dictionary<ulong, (double Start, string Operation)>();
var durations = new SortedDictionary<string, List<double>>();
var statuses = new SortedDictionary<string, long>();
long replacedIrps = 0;
var disk = new SortedDictionary<string, (long Count, long Bytes, double ElapsedMs)>();
var switches = new SortedDictionary<string, long>();
foreach (var e in log.Events)
{
    // Completion can occur on a different thread/process: match IRPs globally.
    if (e is FileIOOpEndTraceData end && pending.Remove(end.IrpPtr, out var start))
    {
        double ms = e.TimeStampRelativeMSec - start.Start;
        if (ms < 0) throw new InvalidDataException("Negative FileIO duration");
        if (!durations.TryGetValue(start.Operation, out var values))
            durations[start.Operation] = values = new List<double>();
        values.Add(ms);
        // Nonzero statuses can be expected, e.g. end-of-directory enumeration.
        string status = $"{start.Operation}:0x{end.NtStatus:X8}";
        statuses[status] = statuses.GetValueOrDefault(status) + 1;
    }
    if (e.ProcessID == process.ProcessID && e.TaskName.Equals("FileIO", StringComparison.OrdinalIgnoreCase))
    {
        string op = e.OpcodeName;
        operations[op] = operations.GetValueOrDefault(op) + 1;
        if (e is not FileIOOpEndTraceData && e.PayloadNames.Contains("IrpPtr"))
        {
            ulong irp = Convert.ToUInt64(e.PayloadByName("IrpPtr"));
            if (irp != 0)
            {
                // DeletePath augments the same delete request; retain its first timestamp.
                if (pending.TryGetValue(irp, out var previous) && previous.Operation == "Delete" &&
                    (op == "DletePath" || op == "DeletePath")) continue;
                if (pending.ContainsKey(irp)) replacedIrps++;
                pending[irp] = (e.TimeStampRelativeMSec, op);
            }
        }
    }
    // Physical completions can be attributed to System. Keep the scope
    // explicit: these are machine-wide events during rip's lifetime.
    if (e.TimeStampRelativeMSec < process.StartTimeRelativeMsec ||
        e.TimeStampRelativeMSec > process.EndTimeRelativeMsec) continue;
    if (e is DiskIOTraceData io && (e.OpcodeName == "Read" || e.OpcodeName == "Write"))
    {
        string key = $"disk={io.DiskNumber} pid={e.ProcessID} {e.OpcodeName}";
        var value = disk.GetValueOrDefault(key);
        disk[key] = (value.Count + 1, value.Bytes + io.TransferSize, value.ElapsedMs + io.ElapsedTimeMSec);
    }
    if (e is CSwitchTraceData cs && cs.OldProcessID == process.ProcessID)
    {
        string key = $"{cs.OldThreadState}:{cs.OldThreadWaitReason}";
        switches[key] = switches.GetValueOrDefault(key) + 1;
    }
}

var summary = new
{
    mode = args[3],
    process = new { process.Name, process.ProcessID, process.CommandLine,
        process.StartTimeRelativeMsec, process.EndTimeRelativeMsec,
        wallMs = process.EndTimeRelativeMsec - process.StartTimeRelativeMsec },
    log.EventsLost,
    cpu = new { sampledCpuMs, samples, samplesWithoutStacks,
        inclusive = inclusive.OrderByDescending(x => x.Value).Take(200).Select(x => new { frame = x.Key, ms = x.Value }),
        exclusive = exclusive.OrderByDescending(x => x.Value).Take(100).Select(x => new { frame = x.Key, ms = x.Value }),
        timeline100ms = cpuTimeline },
    fileIo = new { operations, pendingAtEnd = pending.Count, replacedIrps, statuses,
        durations = durations.Select(x => {
            var sorted = x.Value.Order().ToArray();
            return new {
                operation = x.Key, count = sorted.Length, aggregateMs = sorted.Sum(),
                medianMs = (sorted[(sorted.Length - 1) / 2] + sorted[sorted.Length / 2]) / 2,
                p95Ms = sorted[(int)Math.Ceiling(sorted.Length * .95) - 1],
                maxMs = sorted[^1]
            };
        }) },
    diskIo = disk.Select(x => new { scope = x.Key, x.Value.Count, x.Value.Bytes, x.Value.ElapsedMs }),
    switchOutCounts = switches,
    caveats = new[] {
        "CPU sampling estimates aggregate core-time, not wall time; inclusive frames overlap.",
        "FileIO durations overlap across threads; their sum is not wall time.",
        "DiskIO is machine-wide during rip's lifetime, including background activity.",
        "Context-switch reason counts are not wait durations.",
        "Tracing adds overhead; compare with the separate untraced validation run."
    }
};
File.WriteAllText(args[2], JsonSerializer.Serialize(summary, new JsonSerializerOptions { WriteIndented = true }));
if (log.EventsLost != 0 || samples == 0 || (args[3] == "io" && operations.Count == 0))
    throw new InvalidDataException($"Incomplete trace: lost={log.EventsLost}, samples={samples}, fileOps={operations.Count}");
