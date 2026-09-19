#include "audioprocess/execution_error.h"
#include "audioprocess/graph_validator.h"
#include "audioprocess/sync_graph_executor.h"

#include <cmath>
#include <functional>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>

namespace {
using namespace audioprocess;

static_assert(std::is_const_v<AudioClipPtr::element_type>);
static_assert(!std::is_assignable_v<decltype(std::declval<AudioClipPtr>()->samples[0]), float>);

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}

template <class Function>
ExecutionError expect_error(Function&& function) {
    try {
        function();
    } catch (const ExecutionError& error) {
        require(!error.code.empty(), "Structured failure is missing its code");
        return error;
    }
    throw std::runtime_error("Expected structured rejection, but operation succeeded");
}

using Function = std::function<OutputValues(const InputValues&, ExecutionContext&)>;

class TestNode final : public ISyncNode {
public:
    TestNode(NodeDescriptor descriptor, Function function)
        : descriptor_(std::move(descriptor)), function_(std::move(function)) {}
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        return function_(inputs, context);
    }
private:
    NodeDescriptor descriptor_;
    Function function_;
};

NodeDescriptor describe(std::string type, std::vector<PortDescriptor> inputs,
                        std::vector<PortDescriptor> outputs) {
    return {std::move(type), "Test node", "Independent contract probe",
        ExecutionDomain::Synchronous, std::move(inputs), std::move(outputs), {}};
}

void add(NodeRegistry& registry, NodeDescriptor descriptor, Function function) {
    auto registered = descriptor;
    registry.register_type(std::move(registered),
        [descriptor = std::move(descriptor), function = std::move(function)](const ParameterMap&) {
            return std::make_unique<TestNode>(descriptor, function);
        });
}

GraphDefinition single(std::string type, ParameterMap parameters = {}) {
    GraphDefinition graph;
    graph.nodes.push_back({"probe", std::move(type), std::move(parameters)});
    return graph;
}

void test_schema_normalization_and_validation_without_factories() {
    NodeRegistry registry;
    auto descriptor = describe("schema", {}, {{"value", DataType::Number}});
    ParameterDescriptor gain{"gain", ParameterType::Number, "Test scalar"};
    gain.default_value = 1.0;
    gain.minimum = 0.0;
    gain.maximum = 2.0;
    ParameterDescriptor label{"label", ParameterType::Text, "Required enum"};
    label.required = true;
    label.enum_values = {"a", "b"};
    descriptor.parameters = {gain, label};
    int factory_calls = 0;
    registry.register_type(descriptor, [descriptor, &factory_calls](const ParameterMap& parameters) {
        ++factory_calls;
        const auto value = std::get<double>(parameters.at("gain"));
        return std::make_unique<TestNode>(descriptor,
            [value](const InputValues&, ExecutionContext&) { return OutputValues{{"value", value}}; });
    });

    const auto graph = single("schema", {{"label", std::string("a")}});
    const auto validation = validate_graph(graph, registry);
    require(std::get<double>(validation.graph.nodes[0].parameters.at("gain")) == 1.0,
        "Default parameter was not included in validated graph");
    require(!graph.nodes[0].parameters.contains("gain"), "Validation modified caller graph");
    auto executor = SyncGraphExecutor::compile(graph, registry);
    require(factory_calls == 0, "Validate/compile executed a factory");
    require(std::get<double>(executor.execute().value("probe", "value")) == 1.0,
        "Factory did not receive normalized parameters");
    require(factory_calls == 1, "Execution did not create exactly one instance");

    const auto reject = [&](ParameterMap parameters, const std::string& parameter) {
        const auto error = expect_error([&] {
            static_cast<void>(validate_graph(single("schema", std::move(parameters)), registry));
        });
        require(error.node_id == "probe" && error.parameter_id == parameter,
            "Parameter error did not identify node and parameter");
    };
    reject({}, "label");
    reject({{"label", std::string("invalid")}}, "label");
    reject({{"label", std::string("a")}, {"gaim", 1.0}}, "gaim");
    reject({{"label", std::string("a")}, {"gain", true}}, "gain");
    for (const auto value : {-0.1, 2.1, std::numeric_limits<double>::quiet_NaN(),
                            std::numeric_limits<double>::infinity()}) {
        reject({{"label", std::string("a")}, {"gain", value}}, "gain");
    }
    for (const auto bound : {0.0, 2.0}) {
        static_cast<void>(validate_graph(
            single("schema", {{"label", std::string("b")}, {"gain", bound}}), registry));
    }
    require(factory_calls == 1, "Invalid graph validation invoked a factory");
}

NodeRegistry arithmetic_registry() {
    NodeRegistry registry;
    add(registry, describe("number", {}, {{"out", DataType::Number}}),
        [](const InputValues&, ExecutionContext&) { return OutputValues{{"out", 3.0}}; });
    add(registry, describe("text", {}, {{"out", DataType::Text}}),
        [](const InputValues&, ExecutionContext&) { return OutputValues{{"out", std::string("abc")}}; });
    add(registry, describe("length", {{"in", DataType::Text}}, {{"out", DataType::Number}}),
        [](const InputValues& input, ExecutionContext&) {
            return OutputValues{{"out", static_cast<double>(std::get<std::string>(input.at("in")).size())}};
        });
    add(registry, describe("sum", {{"left", DataType::Number}, {"right", DataType::Number}},
                           {{"out", DataType::Number}}),
        [](const InputValues& input, ExecutionContext&) {
            return OutputValues{{"out", std::get<double>(input.at("left")) +
                std::get<double>(input.at("right"))}};
        });
    return registry;
}

void test_graph_topology_types_and_exports() {
    const auto registry = arithmetic_registry();
    GraphDefinition graph;
    // 故意先写下游，执行顺序必须由连接决定，而不是配置数组顺序。
    graph.nodes = {{"sum", "sum", {}}, {"length", "length", {}}, {"text", "text", {}}};
    graph.connections = {{"text", "out", "length", "in"},
        {"length", "out", "sum", "left"}, {"length", "out", "sum", "right"}};
    graph.exports = {{"score", "sum", "out"}};
    auto executor = SyncGraphExecutor::compile(graph, registry);
    require(std::get<double>(executor.execute().value("sum", "out")) == 6.0,
        "Fan-out into two inputs did not execute after both inputs were ready");

    auto bad = graph;
    bad.connections.pop_back();
    auto error = expect_error([&] { static_cast<void>(validate_graph(bad, registry)); });
    require(error.node_id == "sum" && error.port_id == "right", "Missing input lacks location");
    bad = graph;
    bad.connections.back() = {"text", "out", "sum", "right"};
    error = expect_error([&] { static_cast<void>(validate_graph(bad, registry)); });
    require(error.node_id == "sum" && error.port_id == "right", "Type mismatch lacks target location");
    bad = graph;
    bad.connections.push_back(graph.connections.back());
    static_cast<void>(expect_error([&] { static_cast<void>(validate_graph(bad, registry)); }));
    bad = graph;
    bad.exports[0].port_id = "missing";
    static_cast<void>(expect_error([&] { static_cast<void>(validate_graph(bad, registry)); }));
    bad = graph;
    bad.exports.push_back(bad.exports[0]);
    static_cast<void>(expect_error([&] { static_cast<void>(validate_graph(bad, registry)); }));
    bad = graph;
    bad.schema_version = 999;
    static_cast<void>(expect_error([&] { static_cast<void>(validate_graph(bad, registry)); }));

    GraphDefinition cycle;
    cycle.nodes = {{"source", "number", {}}, {"a", "sum", {}}, {"b", "sum", {}}};
    cycle.connections = {{"source", "out", "a", "left"}, {"source", "out", "b", "left"},
        {"a", "out", "b", "right"}, {"b", "out", "a", "right"}};
    static_cast<void>(expect_error([&] { static_cast<void>(validate_graph(cycle, registry)); }));
}

void test_branch_audio_isolation() {
    NodeRegistry registry;
    auto source = std::make_shared<AudioClip>();
    source->format = {48'000, 1};
    source->samples = {0.25F, -0.5F};
    AudioClipPtr immutable = source;
    add(registry, describe("source", {}, {{"out", DataType::Audio}}),
        [immutable](const InputValues&, ExecutionContext&) { return OutputValues{{"out", immutable}}; });
    add(registry, describe("scale", {{"in", DataType::Audio}}, {{"out", DataType::Audio}}),
        [](const InputValues& input, ExecutionContext&) {
            auto copy = std::make_shared<AudioClip>(*std::get<AudioClipPtr>(input.at("in")));
            for (auto& sample : copy->samples) sample *= 2.0F;
            return OutputValues{{"out", AudioClipPtr(copy)}};
        });
    add(registry, describe("observe", {{"in", DataType::Audio}}, {{"out", DataType::Number}}),
        [](const InputValues& input, ExecutionContext&) {
            return OutputValues{{"out", static_cast<double>(std::get<AudioClipPtr>(input.at("in"))->samples[0])}};
        });
    GraphDefinition graph;
    graph.nodes = {{"s", "source", {}}, {"scaled", "scale", {}}, {"original", "observe", {}}};
    graph.connections = {{"s", "out", "scaled", "in"}, {"s", "out", "original", "in"}};
    auto executor = SyncGraphExecutor::compile(graph, registry);
    const auto result = executor.execute();
    require(std::get<double>(result.value("original", "out")) == 0.25,
        "Writing branch changed its sibling input");
    const auto scaled = std::get<AudioClipPtr>(result.value("scaled", "out"));
    require(scaled->samples[0] == 0.5F && scaled->samples[1] == -1.0F,
        "Writing branch did not create the expected independent output");
}

void test_task_state_does_not_leak() {
    NodeRegistry registry;
    auto descriptor = describe("counter", {}, {{"out", DataType::Number}});
    registry.register_type(descriptor, [descriptor](const ParameterMap&) {
        return std::make_unique<TestNode>(descriptor,
            [count = 0](const InputValues&, ExecutionContext&) mutable {
                return OutputValues{{"out", static_cast<double>(++count)}};
            });
    });
    auto executor = SyncGraphExecutor::compile(single("counter"), registry);
    require(std::get<double>(executor.execute().value("probe", "out")) == 1.0 &&
            std::get<double>(executor.execute().value("probe", "out")) == 1.0,
        "A new task inherited state from a previous node instance");
}

void test_factory_must_match_registered_contract() {
    const auto registered = describe("factory", {{"in", DataType::Number, false}},
        {{"first", DataType::Number}, {"second", DataType::Text}});
    const auto create_with = [&](const NodeDescriptor& actual) {
        NodeRegistry registry;
        registry.register_type(registered, [actual](const ParameterMap&) {
            return std::make_unique<TestNode>(actual,
                [](const InputValues&, ExecutionContext&) { return OutputValues{}; });
        });
        return registry.create("factory", {});
    };
    const auto reject = [&](const NodeDescriptor& actual) {
        const auto error = expect_error([&] { static_cast<void>(create_with(actual)); });
        require(error.code == "invalid_factory", "Factory contract mismatch used the wrong error");
    };
    auto changed = registered;
    changed.execution_domain = ExecutionDomain::Asynchronous;
    reject(changed);
    changed = registered;
    changed.inputs[0].type = DataType::Text;
    reject(changed);
    changed = registered;
    changed.inputs[0].required = true;
    reject(changed);
    changed = registered;
    changed.outputs[0].id = "renamed";
    reject(changed);
    changed = registered;
    changed.outputs.pop_back();
    reject(changed);
    // 端口按稳定 ID 匹配；展示文案及向量顺序不应阻止等价实例使用。
    changed = registered;
    std::swap(changed.outputs[0], changed.outputs[1]);
    changed.display_name = "Equivalent implementation";
    require(create_with(changed) != nullptr, "Equivalent reordered port contract was rejected");
}

void test_output_contract_failures() {
    const auto rejects_output = [](DataType type, OutputValues output) {
        NodeRegistry registry;
        add(registry, describe("bad", {}, {{"out", type}}),
            [output](const InputValues&, ExecutionContext&) { return output; });
        auto executor = SyncGraphExecutor::compile(single("bad"), registry);
        const auto error = expect_error([&] { static_cast<void>(executor.execute()); });
        require(error.node_id == "probe", "Node output error did not identify its node");
    };
    rejects_output(DataType::Number, {});
    rejects_output(DataType::Number, {{"out", std::string("wrong")}});
    rejects_output(DataType::Number, {{"out", std::numeric_limits<double>::quiet_NaN()}});
    rejects_output(DataType::Number, {{"out", 1.0}, {"undeclared", 2.0}});
    rejects_output(DataType::Audio, {{"out", AudioClipPtr{}}});
    auto malformed = std::make_shared<AudioClip>();
    malformed->format = {48'000, 2};
    malformed->samples = {0.5F};
    rejects_output(DataType::Audio, {{"out", AudioClipPtr(malformed)}});
    malformed = std::make_shared<AudioClip>();
    malformed->format = {48'000, 1};
    malformed->samples = {std::numeric_limits<float>::infinity()};
    rejects_output(DataType::Audio, {{"out", AudioClipPtr(malformed)}});
}

void test_cancellation_prevents_downstream_and_final_publication() {
    std::atomic_bool cancelled{true};
    int invocations = 0;
    NodeRegistry registry;
    add(registry, describe("cancel", {}, {{"out", DataType::Number}}),
        [&](const InputValues&, ExecutionContext&) {
            ++invocations;
            cancelled.store(true);
            return OutputValues{{"out", 1.0}};
        });
    add(registry, describe("downstream", {{"in", DataType::Number}}, {{"out", DataType::Number}}),
        [&](const InputValues&, ExecutionContext&) {
            ++invocations;
            return OutputValues{{"out", 2.0}};
        });
    auto executor = SyncGraphExecutor::compile(single("cancel"), registry);
    static_cast<void>(expect_error([&] { static_cast<void>(executor.execute({&cancelled})); }));
    require(invocations == 0, "Already cancelled task executed a node");
    cancelled.store(false);
    static_cast<void>(expect_error([&] { static_cast<void>(executor.execute({&cancelled})); }));
    require(invocations == 1, "Cancellation at last node was not observed");

    auto graph = single("cancel");
    graph.nodes.push_back({"down", "downstream", {}});
    graph.connections.push_back({"probe", "out", "down", "in"});
    auto with_downstream = SyncGraphExecutor::compile(graph, registry);
    cancelled.store(false);
    static_cast<void>(expect_error([&] { static_cast<void>(with_downstream.execute({&cancelled})); }));
    require(invocations == 2, "Cancelled task still executed downstream work");
}

}  // namespace

int main() {
    try {
        test_schema_normalization_and_validation_without_factories();
        test_graph_topology_types_and_exports();
        test_branch_audio_isolation();
        test_task_state_does_not_leak();
        test_factory_must_match_registered_contract();
        test_output_contract_failures();
        test_cancellation_prevents_downstream_and_final_publication();
        std::cout << "Independent graph contract tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Contract test failure: " << error.what() << '\n';
        return 1;
    }
}
