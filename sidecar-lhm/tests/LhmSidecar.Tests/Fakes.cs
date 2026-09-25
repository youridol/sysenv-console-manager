// Fakes.cs —— LibreHardwareMonitor 接口的最小测试替身。
//
// 只实现测试断言需要的成员：Name / HardwareType / Sensors / SubHardware / Value /
// Identifier / Update()。用于在无硬件、无 ring0 权限的环境下验证 LiteMonitor 映射语义。

using LibreHardwareMonitor.Hardware;

namespace LhmSidecar.Tests;

/// <summary>假传感器。</summary>
internal sealed class FakeSensor : ISensor
{
    private float? _value;

    public FakeSensor(string name, SensorType type, float? value, IHardware? hardware = null)
    {
        Name = name;
        SensorType = type;
        _value = value;
        Hardware = hardware;
    }

    public string Name { get; set; }
    public SensorType SensorType { get; set; }
    public float? Value { get => _value; set => _value = value; }
    public IControl? Control { get; set; }
    public IHardware? Hardware { get; set; }
    public Identifier Identifier { get; } = new Identifier("fake", "sensor");
    public int Index { get; set; }
    public bool IsDefaultHidden => false;
    public float? Max => _value;
    public float? Min => _value;
    public IReadOnlyList<IParameter> Parameters => Array.Empty<IParameter>();
    public IEnumerable<SensorValue> Values => Array.Empty<SensorValue>();
    public TimeSpan ValuesTimeWindow { get; set; } = TimeSpan.Zero;
    public void ResetMin() { }
    public void ResetMax() { }
    public void ClearValues() { }

    // IElement：测试不需要遍历分派
    public void Accept(IVisitor visitor) { }
    public void Traverse(IVisitor visitor) { }
}

/// <summary>假硬件节点（可含子硬件）。</summary>
internal sealed class FakeHardware : IHardware
{
    private readonly List<ISensor> _sensors = new();
    private readonly List<IHardware> _sub = new();

    public FakeHardware(string name, HardwareType type)
    {
        Name = name;
        HardwareType = type;
        Identifier = new Identifier("fake", type.ToString(), name);
    }

    public HardwareType HardwareType { get; set; }
    public string Name { get; set; }
    public Identifier Identifier { get; }
    public IHardware? Parent { get; set; }
    public ISensor[] Sensors => _sensors.ToArray();
    public IHardware[] SubHardware => _sub.ToArray();
    public IDictionary<string, string> Properties { get; } = new Dictionary<string, string>();

    public int UpdateCount { get; private set; }
    public string GetReport() => string.Empty;
    public void Update() => UpdateCount++;

    // IElement：测试不需要遍历分派
    public void Accept(IVisitor visitor) { }
    public void Traverse(IVisitor visitor) { }

    public event SensorEventHandler? SensorAdded;
    public event SensorEventHandler? SensorRemoved;

    /// <summary>挂一个传感器，并把 Hardware 反向指向本节点（LHM 真实行为）。</summary>
    public FakeHardware With(string sensorName, SensorType type, float? value)
    {
        var s = new FakeSensor(sensorName, type, value, this);
        _sensors.Add(s);
        return this;
    }

    /// <summary>挂一个已构造好的传感器（用于断言"哪一个"被选中）。</summary>
    public FakeSensor WithSensor(FakeSensor sensor)
    {
        sensor.Hardware = this;
        _sensors.Add(sensor);
        return sensor;
    }

    public FakeHardware WithSub(FakeHardware child)
    {
        child.Parent = this;
        _sub.Add(child);
        return this;
    }

    internal void RaiseNothing() { _ = SensorAdded; _ = SensorRemoved; }
}

/// <summary>假 Computer（Hardware 列表可控）。</summary>
internal sealed class FakeComputer : IComputer
{
    public IList<IHardware> Hardware { get; } = new List<IHardware>();

    public bool IsBatteryEnabled => true;
    public bool IsControllerEnabled => true;
    public bool IsCpuEnabled => true;
    public bool IsGpuEnabled => true;
    public bool IsPowerMonitorEnabled => false;
    public bool IsMemoryEnabled => true;
    public bool IsMotherboardEnabled => true;
    public bool IsNetworkEnabled => true;
    public bool IsPsuEnabled => false;
    public bool IsStorageEnabled => true;

    public string GetReport() => string.Empty;

    // IElement：测试不需要遍历分派
    public void Accept(IVisitor visitor) { }
    public void Traverse(IVisitor visitor) { }

    public event HardwareEventHandler? HardwareAdded;
    public event HardwareEventHandler? HardwareRemoved;

    public FakeComputer With(FakeHardware hw)
    {
        Hardware.Add(hw);
        return this;
    }

    internal void RaiseNothing() { _ = HardwareAdded; _ = HardwareRemoved; }
}
