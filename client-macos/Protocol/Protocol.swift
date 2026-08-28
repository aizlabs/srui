import Foundation
@_exported import SwiftProtobuf

/// Namespace 0 is permanently reserved for the canonical SRUI standard registry (§6.4).
public let standardNamespaceID: UInt32 = 0

// Convenient typealiases for SRUI wire types
public typealias SRUINodeRecord = Srui_Protocol_NodeRecord
public typealias SRUITypeRef = Srui_Protocol_TypeRef
public typealias SRUIPropertyRef = Srui_Protocol_PropertyRef
public typealias SRUIProperty = Srui_Protocol_Property
public typealias SRUIValue = Srui_Protocol_Value
public typealias SRUIOperation = Srui_Protocol_Operation
public typealias SRUICommitOp = Srui_Protocol_CommitOp
public typealias SRUITransaction = Srui_Protocol_Transaction
public typealias SRUIEvent = Srui_Protocol_Event
public typealias SRUIStandardEnum = Srui_Protocol_StandardEnum
public typealias SRUIStandardNodeType = Srui_Protocol_StandardNodeType
public typealias SRUIStandardProperty = Srui_Protocol_StandardProperty
public typealias SRUIStandardEvent = Srui_Protocol_StandardEvent
public typealias SRUIStandardOperation = Srui_Protocol_StandardOperation
public typealias SRUIClientHello = Srui_Protocol_ClientHello
public typealias SRUIServerWelcome = Srui_Protocol_ServerWelcome
public typealias SRUIClientResume = Srui_Protocol_ClientResume
public typealias SRUIServerResumeOk = Srui_Protocol_ServerResumeOk
public typealias SRUIServerResyncRequired = Srui_Protocol_ServerResyncRequired
public typealias SRUIMessage = Srui_Protocol_SruiMessage

public struct ProtocolPlaceholder {
    public init() {}
}
