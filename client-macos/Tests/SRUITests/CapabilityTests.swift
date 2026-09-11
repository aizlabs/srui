//
// CapabilityTests.swift
// SRUITests
//
// Conformance and unit tests for capability profiles, sets, and negotiation logic (§4 inv. 13, §6.1, §6.4, §11, §15).
//

import Testing
import Foundation
import SemanticModel

@Suite("Capability & Profile Negotiation Tests (§4 inv. 13, §15)")
struct CapabilityTests {

    // MARK: - Profile Parsing & Validation

    @Test("Valid profile string parses name and version correctly")
    func validProfileParsing() throws {
        let p = try Profile.parse("org.srui.standard-widgets/1")
        #expect(p.name == "org.srui.standard-widgets")
        #expect(p.version == 1)
        #expect(p.description == "org.srui.standard-widgets/1")

        let terminal = try Profile.parse("org.srui.terminal/2")
        #expect(terminal.name == "org.srui.terminal")
        #expect(terminal.version == 2)
        #expect(terminal.description == "org.srui.terminal/2")
    }

    @Test("Profile constructor validates non-empty name and positive version")
    func profileConstructorValidation() throws {
        #expect(throws: ParseProfileError.emptyName) {
            try Profile(name: "", version: 1)
        }
        #expect(throws: ParseProfileError.emptyName) {
            try Profile(name: "   ", version: 1)
        }
        #expect(throws: ParseProfileError.invalidVersion("version must be >= 1")) {
            try Profile(name: "org.srui.test", version: 0)
        }

        let valid = try Profile(name: "org.srui.test", version: 5)
        #expect(valid.name == "org.srui.test")
        #expect(valid.version == 5)
    }

    @Test("Invalid profile strings throw descriptive ParseProfileError")
    func invalidProfileParsing() {
        #expect(throws: ParseProfileError.emptyString) {
            try Profile.parse("")
        }
        #expect(throws: ParseProfileError.emptyString) {
            try Profile.parse("   ")
        }
        #expect(throws: ParseProfileError.missingVersionDelimiter("org.srui.standard-widgets")) {
            try Profile.parse("org.srui.standard-widgets")
        }
        #expect(throws: ParseProfileError.emptyName) {
            try Profile.parse("/1")
        }
        #expect(throws: ParseProfileError.invalidVersion("0")) {
            try Profile.parse("org.srui.test/0")
        }
        #expect(throws: ParseProfileError.invalidVersion("abc")) {
            try Profile.parse("org.srui.test/abc")
        }
        #expect(throws: ParseProfileError.invalidVersion("-1")) {
            try Profile.parse("org.srui.test/-1")
        }
    }

    @Test("Standard profile constants match protocol specifications")
    func standardProfileConstants() {
        #expect(Profile.standardWidgetsV1.description == "org.srui.standard-widgets/1")
        #expect(Profile.terminalV1.description == "org.srui.terminal/1")
        #expect(Profile.richtextV1.description == "org.srui.richtext/1")
        #expect(Profile.vectorSceneV1.description == "org.srui.vector-scene/1")
        #expect(Profile.mediaSurfaceV1.description == "org.srui.media-surface/1")
        #expect(Profile.codingV1.description == "org.srui.coding/1")
    }

    @Test("Profile equality, hashing, and sorting")
    func profileSetBehavior() {
        let p1 = Profile.standardWidgetsV1
        let p2 = try? Profile.parse("org.srui.standard-widgets/1")
        #expect(p1 == p2)
        #expect(p1.hashValue == p2?.hashValue)

        let p3 = Profile.terminalV1
        #expect(p1 != p3)

        let sorted = [Profile.terminalV1, Profile.standardWidgetsV1, Profile.codingV1].sorted()
        #expect(sorted == [Profile.codingV1, Profile.standardWidgetsV1, Profile.terminalV1])
    }

    // MARK: - CapabilitySet Operations

    @Test("CapabilitySet insertion, removal, and membership queries")
    func capabilitySetBasicOps() throws {
        var set = CapabilitySet()
        #expect(set.isEmpty)
        #expect(set.count == 0)

        let (inserted1, _) = set.insert(Profile.standardWidgetsV1)
        #expect(inserted1)
        let (inserted2, _) = set.insert(Profile.standardWidgetsV1)
        #expect(inserted2 == false)
        #expect(set.count == 1)
        #expect(set.contains(Profile.standardWidgetsV1))
        #expect(set.contains(string: "org.srui.standard-widgets/1"))
        #expect(set.contains(name: "org.srui.standard-widgets"))
        #expect(set.getVersion(named: "org.srui.standard-widgets") == 1)

        let removed1 = set.remove(Profile.standardWidgetsV1)
        #expect(removed1 == Profile.standardWidgetsV1)
        let removed2 = set.remove(Profile.standardWidgetsV1)
        #expect(removed2 == nil)
        #expect(set.isEmpty)
    }

    @Test("CapabilitySet set algebra: intersection, union, difference, sequence")
    func capabilitySetAlgebra() {
        let setA: CapabilitySet = [Profile.standardWidgetsV1, Profile.terminalV1]
        let setB: CapabilitySet = [Profile.terminalV1, Profile.richtextV1]

        let inter = setA.intersection(setB)
        #expect(inter == [Profile.terminalV1])

        let union = setA.union(setB)
        #expect(union == [Profile.standardWidgetsV1, Profile.terminalV1, Profile.richtextV1])

        let diff = setA.subtracting(setB)
        #expect(diff == [Profile.standardWidgetsV1])

        let symDiff = setA.symmetricDifference(setB)
        #expect(symDiff == [Profile.standardWidgetsV1, Profile.richtextV1])

        var mutatingSet = setA
        mutatingSet.formIntersection(setB)
        #expect(mutatingSet == [Profile.terminalV1])

        #expect(setA.isSuperset(of: [Profile.terminalV1]))
        let subsetTest: CapabilitySet = [Profile.terminalV1]
        #expect(subsetTest.isSubset(of: setA))

        // Sequence iteration
        var iterated = [Profile]()
        for profile in setA {
            iterated.append(profile)
        }
        #expect(iterated.count == 2)
    }

    // MARK: - Capability Negotiation (§15, §4 Invariant 13)

    @Test("Negotiation succeeds when client offers all required profiles")
    func negotiationSuccessWithMatchingRequired() throws {
        let clientOffered: CapabilitySet = [
            Profile.standardWidgetsV1,
            Profile.terminalV1,
            Profile.richtextV1
        ]
        let serverRequired: CapabilitySet = [Profile.standardWidgetsV1]
        let serverOptional: CapabilitySet = [Profile.terminalV1]

        let negotiated = try CapabilitySet.negotiate(
            clientOffered: clientOffered,
            serverRequired: serverRequired,
            serverOptional: serverOptional
        )

        #expect(negotiated.contains(Profile.standardWidgetsV1))
        #expect(negotiated.contains(Profile.terminalV1))
        #expect(negotiated.contains(Profile.richtextV1) == false)
    }

    @Test("Negotiation fails explicitly when client lacks a required profile (§4 inv. 13)")
    func negotiationFailsOnMissingRequired() {
        let clientOffered: CapabilitySet = [Profile.terminalV1]
        let serverRequired: CapabilitySet = [Profile.standardWidgetsV1]
        let serverOptional: CapabilitySet = [Profile.terminalV1]

        #expect(throws: NegotiationError.unsatisfiedRequiredProfiles(missing: [Profile.standardWidgetsV1])) {
            try CapabilitySet.negotiate(
                clientOffered: clientOffered,
                serverRequired: serverRequired,
                serverOptional: serverOptional
            )
        }
    }

    @Test("Negotiation ignores unsupported optional profiles without failing")
    func negotiationIgnoresUnsupportedOptional() throws {
        let clientOffered: CapabilitySet = [Profile.standardWidgetsV1]
        let serverRequired: CapabilitySet = [Profile.standardWidgetsV1]
        let serverOptional: CapabilitySet = [Profile.terminalV1, Profile.vectorSceneV1]

        let negotiated = try CapabilitySet.negotiate(
            clientOffered: clientOffered,
            serverRequired: serverRequired,
            serverOptional: serverOptional
        )

        #expect(negotiated == [Profile.standardWidgetsV1])
    }

    @Test("Standard server capabilities do not advertise unused extensions")
    func standardServerCapabilitiesAreStandardOnly() throws {
        let serverCaps = ServerCapabilities.standardWidgets
        let clientOffered: CapabilitySet = [Profile.standardWidgetsV1, Profile.terminalV1]

        #expect(serverCaps.required == [Profile.standardWidgetsV1])
        #expect(serverCaps.optional.isEmpty)
        let negotiated = try serverCaps.negotiate(clientOffered: clientOffered)
        #expect(negotiated == [Profile.standardWidgetsV1])
    }

    @Test("CapabilitySet fromStrings and fromValidStrings handling")
    func fromStringsHandling() throws {
        let validStrings = ["org.srui.standard-widgets/1", "org.srui.terminal/1"]
        let set1 = try CapabilitySet.fromStrings(validStrings)
        #expect(set1.count == 2)
        #expect(set1.contains(Profile.standardWidgetsV1))

        let mixedStrings = ["org.srui.standard-widgets/1", "invalid-profile-no-version", "org.srui.terminal/1"]
        #expect(throws: ParseProfileError.self) {
            try CapabilitySet.fromStrings(mixedStrings)
        }

        let setValidOnly = CapabilitySet.fromValidStrings(mixedStrings)
        #expect(setValidOnly.count == 2)
        #expect(setValidOnly.contains(Profile.standardWidgetsV1))
        #expect(setValidOnly.contains(Profile.terminalV1))
    }

    @Test("NegotiationError and ParseProfileError descriptive messages")
    func errorDescriptions() {
        let missing = [Profile.standardWidgetsV1, Profile.terminalV1]
        let err = NegotiationError.unsatisfiedRequiredProfiles(missing: missing)
        #expect(err.description.contains("org.srui.standard-widgets/1"))
        #expect(err.description.contains("org.srui.terminal/1"))

        #expect(ParseProfileError.emptyString.description == "profile string is empty")
        #expect(ParseProfileError.emptyName.description == "profile name is empty")
        #expect(ParseProfileError.missingVersionDelimiter("bad").description.contains("missing version delimiter"))
        #expect(ParseProfileError.invalidVersion("x").description.contains("invalid profile version"))
    }

    @Test("CapabilitySet insert string and update with Profile")
    func insertStringAndUpdate() throws {
        var set = CapabilitySet()
        let inserted = try set.insert(string: "org.srui.standard-widgets/1")
        #expect(inserted)
        let dup = try set.insert(string: "org.srui.standard-widgets/1")
        #expect(!dup)

        let updated = set.update(with: Profile.standardWidgetsV1)
        #expect(updated == Profile.standardWidgetsV1)
    }
}
